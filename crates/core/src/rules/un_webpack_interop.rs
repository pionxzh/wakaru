use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BindingIdent, BlockStmtOrExpr, CallExpr, Callee, Expr, Ident,
    IdentName, IfStmt, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleItem, Pat, ReturnStmt, SimpleAssignTarget, Stmt, VarDecl, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::collect_pat_names;
use super::helper_matcher::{
    binding_key, ident_matches_binding, var_declarator_binding_key, BindingKey,
};
use super::rename_utils::{
    binding_replacement_would_be_shadowed, collect_module_names, rename_bindings_in_module,
    BindingRename,
};
use crate::utils::paren::strip_parens;

/// Removes webpack's interop getter wrappers and replaces their usage with the
/// underlying require binding.
///
/// Webpack emits a getter function that checks `__esModule` and returns either
/// `mod.default` (for ES modules) or `mod` (for plain CJS). The getter is
/// typically a zero-parameter arrow:
///
/// ```js
/// var _lib = require("./lib");
/// var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
/// // block form:
/// var _lib3 = () => { if (_lib && _lib.__esModule) { return _lib.default; } return _lib; };
/// ```
///
/// Call sites use either `_lib2()` (invoke the getter) or `_lib2.a` (webpack's
/// `.a` shorthand for the same thing).
///
/// This rule:
/// 1. Identifies require bindings (`var x = require(…)`)
/// 2. Finds getter declarations whose body matches the interop pattern
/// 3. Verifies every usage of the getter is a safe form (`getter()` or `getter.a`)
/// 4. Replaces each safe usage with the underlying require binding
/// 5. Removes the now-dead getter declaration
///
/// Runs twice in the pipeline (as `UnWebpackInterop` and `UnWebpackInterop2`)
/// to catch getters that only become visible after other rules simplify the AST.
pub struct UnWebpackInterop {
    unresolved_mark: Mark,
}

impl UnWebpackInterop {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

#[derive(Default)]
struct UsageStats {
    supported: usize,
    unsupported: bool,
}

impl VisitMut for UnWebpackInterop {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut has_own_replacer = WebpackHasOwnReplacer {
            unresolved_mark: self.unresolved_mark,
        };
        module.visit_mut_with(&mut has_own_replacer);

        let module_bindings = collect_module_bindings(module, self.unresolved_mark);
        if module_bindings.is_empty() {
            return;
        }

        let initial_ref_counts = collect_binding_ref_counts(module);
        let mut namespace_replacer = WebpackNamespaceReplacer {
            initial_ref_counts: &initial_ref_counts,
            module_bindings: &module_bindings,
            removed_caches: HashSet::new(),
            unresolved_mark: self.unresolved_mark,
        };
        module.visit_mut_with(&mut namespace_replacer);
        remove_unused_namespace_cache_decls(module, &namespace_replacer.removed_caches);

        let mut candidates: HashMap<BindingKey, Ident> = HashMap::new();
        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var))) = item else {
                continue;
            };
            for decl in &var.decls {
                let Pat::Ident(binding) = &decl.name else {
                    continue;
                };
                let Some(init) = &decl.init else {
                    continue;
                };
                if let Some(base) =
                    match_interop_getter(init.as_ref(), &module_bindings).or_else(|| {
                        match_require_n_getter(
                            init.as_ref(),
                            &module_bindings,
                            self.unresolved_mark,
                        )
                    })
                {
                    candidates.insert(binding_key(&binding.id), base);
                }
            }
        }

        if candidates.is_empty() {
            return;
        }

        let mut usage: HashMap<BindingKey, UsageStats> = candidates
            .keys()
            .map(|key| (key.clone(), UsageStats::default()))
            .collect();

        for item in &module.body {
            let mut collector = GetterUsageCollector { usage: &mut usage };
            collector.visit_item(item);
        }

        let to_inline: HashMap<BindingKey, Ident> = candidates
            .into_iter()
            .filter(|(key, _)| {
                usage
                    .get(key)
                    .map(|stats| stats.supported >= 1 && !stats.unsupported)
                    .unwrap_or(false)
            })
            .collect();

        if to_inline.is_empty() {
            return;
        }

        let mut to_inline = to_inline;
        let renames = build_shadow_avoidance_renames(module, &mut to_inline);
        if !renames.is_empty() {
            rename_bindings_in_module(module, &renames);
        }

        let mut new_body = Vec::with_capacity(module.body.len());
        for item in module.body.drain(..) {
            match item {
                ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(mut var))) => {
                    var.decls
                        .retain(|decl| !should_remove_decl(decl, &to_inline));
                    if !var.decls.is_empty() {
                        new_body.push(ModuleItem::Stmt(Stmt::Decl(
                            swc_core::ecma::ast::Decl::Var(var),
                        )));
                    }
                }
                other => new_body.push(other),
            }
        }
        module.body = new_body;

        let mut replacer = GetterReplacer { map: &to_inline };
        module.visit_mut_with(&mut replacer);
    }
}

fn collect_module_bindings(module: &Module, unresolved_mark: Mark) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Named(named) => &named.local,
                        ImportSpecifier::Default(default) => &default.local,
                        ImportSpecifier::Namespace(namespace) => &namespace.local,
                    };
                    bindings.insert(binding_key(local));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let Some(init) = &decl.init else {
                        continue;
                    };
                    if is_require_call(init.as_ref(), unresolved_mark) {
                        bindings.insert(binding_key(&binding.id));
                    }
                }
            }
            _ => {}
        }
    }
    bindings
}

fn is_require_call(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(
        callee.as_ref(),
        Expr::Ident(id)
            if id.sym.as_ref() == "require" && id.ctxt.outer() == unresolved_mark
    )
}

fn match_interop_getter(expr: &Expr, require_bindings: &HashSet<BindingKey>) -> Option<Ident> {
    let Expr::Arrow(arrow) = expr else {
        return None;
    };
    if !arrow.params.is_empty() {
        return None;
    }
    let base = match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(body) => match_interop_cond(body.as_ref(), require_bindings),
        BlockStmtOrExpr::BlockStmt(block) => match_interop_block(block, require_bindings),
    }?;
    Some(base)
}

fn match_require_n_getter(
    expr: &Expr,
    module_bindings: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<Ident> {
    let Expr::Call(call) = expr else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(require_ident) = member.obj.as_ref() else {
        return None;
    };
    if require_ident.sym.as_ref() != "require" || require_ident.ctxt.outer() != unresolved_mark {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "n") {
        return None;
    }
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Expr::Ident(base) = call.args[0].expr.as_ref() else {
        return None;
    };
    if !module_bindings.contains(&binding_key(base)) {
        return None;
    }
    Some(base.clone())
}

struct NamespaceMatch {
    base: Ident,
    cache: Option<BindingKey>,
}

fn match_require_t_namespace(
    expr: &Expr,
    module_bindings: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<NamespaceMatch> {
    let expr = strip_parens(expr);
    match expr {
        Expr::Call(call) => match_require_t_call(call, module_bindings, unresolved_mark)
            .map(|base| NamespaceMatch { base, cache: None }),
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            let AssignTarget::Simple(SimpleAssignTarget::Ident(cache)) = &assign.left else {
                return None;
            };
            match_require_t_namespace(assign.right.as_ref(), module_bindings, unresolved_mark).map(
                |mut namespace| {
                    namespace.cache = Some(binding_key(&cache.id));
                    namespace
                },
            )
        }
        Expr::Bin(bin) if bin.op == swc_core::ecma::ast::BinaryOp::LogicalOr => {
            let namespace =
                match_require_t_namespace(bin.right.as_ref(), module_bindings, unresolved_mark)?;
            let Some(cache) = &namespace.cache else {
                return None;
            };
            let Expr::Ident(left) = strip_parens(bin.left.as_ref()) else {
                return None;
            };
            if !ident_matches_binding(left, cache) {
                return None;
            }
            Some(namespace)
        }
        _ => None,
    }
}

fn match_require_t_call(
    call: &swc_core::ecma::ast::CallExpr,
    module_bindings: &HashSet<BindingKey>,
    unresolved_mark: Mark,
) -> Option<Ident> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Ident(require_ident) = member.obj.as_ref() else {
        return None;
    };
    if require_ident.sym.as_ref() != "require" || require_ident.ctxt.outer() != unresolved_mark {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "t") {
        return None;
    }
    if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let Expr::Ident(base) = call.args[0].expr.as_ref() else {
        return None;
    };
    if !module_bindings.contains(&binding_key(base)) {
        return None;
    }
    if !matches!(call.args[1].expr.as_ref(), Expr::Lit(Lit::Num(mode)) if mode.value == 2.0) {
        return None;
    }
    Some(base.clone())
}

fn static_member_prop_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(prop) => Some(prop.sym.as_ref()),
        MemberProp::Computed(computed) => static_string_expr(computed.expr.as_ref()),
        _ => None,
    }
}

fn static_string_expr(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Lit(Lit::Str(value)) => value.value.as_str(),
        Expr::Call(call) if call.args.is_empty() => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Member(member) = callee.as_ref() else {
                return None;
            };
            if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "toString") {
                return None;
            }
            let Expr::Lit(Lit::Str(value)) = member.obj.as_ref() else {
                return None;
            };
            value.value.as_str()
        }
        _ => None,
    }
}

fn remove_unused_namespace_cache_decls(module: &mut Module, caches: &HashSet<BindingKey>) {
    if caches.is_empty() {
        return;
    }

    let refs = collect_binding_refs(module, caches);
    let removable: HashSet<_> = caches.difference(&refs).cloned().collect();
    if removable.is_empty() {
        return;
    }

    let mut new_body = Vec::with_capacity(module.body.len());
    for item in module.body.drain(..) {
        match item {
            ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(mut var))) => {
                var.decls
                    .retain(|decl| !is_empty_decl_for_binding(decl, &removable));
                if !var.decls.is_empty() {
                    new_body.push(ModuleItem::Stmt(Stmt::Decl(
                        swc_core::ecma::ast::Decl::Var(var),
                    )));
                }
            }
            other => new_body.push(other),
        }
    }
    module.body = new_body;
}

fn collect_binding_refs(module: &Module, targets: &HashSet<BindingKey>) -> HashSet<BindingKey> {
    struct RefCollector<'a> {
        targets: &'a HashSet<BindingKey>,
        refs: HashSet<BindingKey>,
    }

    impl Visit for RefCollector<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            let key = binding_key(ident);
            if self.targets.contains(&key) {
                self.refs.insert(key);
            }
        }

        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

        fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}
    }

    let mut collector = RefCollector {
        targets,
        refs: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.refs
}

fn is_empty_decl_for_binding(decl: &VarDeclarator, bindings: &HashSet<BindingKey>) -> bool {
    if decl.init.is_some() {
        return false;
    }
    var_declarator_binding_key(decl).is_some_and(|key| bindings.contains(&key))
}

fn collect_binding_ref_counts(module: &Module) -> HashMap<BindingKey, usize> {
    struct RefCounter {
        refs: HashMap<BindingKey, usize>,
    }

    impl Visit for RefCounter {
        fn visit_ident(&mut self, ident: &Ident) {
            *self.refs.entry(binding_key(ident)).or_insert(0) += 1;
        }

        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

        fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}
    }

    let mut counter = RefCounter {
        refs: HashMap::new(),
    };
    module.visit_with(&mut counter);
    counter.refs
}

fn count_binding_refs_in_expr(expr: &Expr, target: &BindingKey) -> usize {
    struct RefCounter<'a> {
        target: &'a BindingKey,
        refs: usize,
    }

    impl Visit for RefCounter<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            if ident_matches_binding(ident, self.target) {
                self.refs += 1;
            }
        }

        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

        fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}
    }

    let mut counter = RefCounter { target, refs: 0 };
    expr.visit_with(&mut counter);
    counter.refs
}

fn match_interop_cond(expr: &Expr, require_bindings: &HashSet<BindingKey>) -> Option<Ident> {
    let Expr::Cond(cond) = expr else {
        return None;
    };
    let Expr::Bin(test) = cond.test.as_ref() else {
        return None;
    };
    if test.op != swc_core::ecma::ast::BinaryOp::LogicalAnd {
        return None;
    }

    let Expr::Ident(base) = test.left.as_ref() else {
        return None;
    };
    let base_key = binding_key(base);
    if !require_bindings.contains(&base_key) {
        return None;
    }

    if !matches_esmodule_member(test.right.as_ref(), base) {
        return None;
    }
    if !matches_default_member(cond.cons.as_ref(), base) {
        return None;
    }
    let Expr::Ident(alt_ident) = cond.alt.as_ref() else {
        return None;
    };
    if !ident_matches_binding(alt_ident, &base_key) {
        return None;
    }

    Some(base.clone())
}

fn match_interop_block(
    block: &swc_core::ecma::ast::BlockStmt,
    require_bindings: &HashSet<BindingKey>,
) -> Option<Ident> {
    // Form A: { return cond ? cons : alt; }  (single return of ternary)
    if block.stmts.len() == 1 {
        if let Stmt::Return(ReturnStmt {
            arg: Some(ret_arg), ..
        }) = &block.stmts[0]
        {
            return match_interop_cond(ret_arg.as_ref(), require_bindings);
        }
    }

    // Form B: { if (test) { return cons; } return alt; }  (two statements)
    if block.stmts.len() != 2 {
        return None;
    }

    let Stmt::If(IfStmt {
        test, cons, alt, ..
    }) = &block.stmts[0]
    else {
        return None;
    };
    if alt.is_some() {
        return None;
    }
    let Expr::Bin(test_bin) = test.as_ref() else {
        return None;
    };
    if test_bin.op != swc_core::ecma::ast::BinaryOp::LogicalAnd {
        return None;
    }
    let Expr::Ident(base) = test_bin.left.as_ref() else {
        return None;
    };
    let base_key = binding_key(base);
    if !require_bindings.contains(&base_key) {
        return None;
    }
    if !matches_esmodule_member(test_bin.right.as_ref(), base) {
        return None;
    }

    let Stmt::Block(cons_block) = cons.as_ref() else {
        return None;
    };
    if cons_block.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ReturnStmt {
        arg: Some(cons_arg),
        ..
    }) = &cons_block.stmts[0]
    else {
        return None;
    };
    if !matches_default_member(cons_arg.as_ref(), base) {
        return None;
    }

    let Stmt::Return(ReturnStmt {
        arg: Some(alt_arg), ..
    }) = &block.stmts[1]
    else {
        return None;
    };
    let Expr::Ident(alt_ident) = alt_arg.as_ref() else {
        return None;
    };
    if !ident_matches_binding(alt_ident, &base_key) {
        return None;
    }

    Some(base.clone())
}

fn matches_esmodule_member(expr: &Expr, base: &Ident) -> bool {
    matches_member(expr, base, "__esModule")
}

fn matches_default_member(expr: &Expr, base: &Ident) -> bool {
    matches_member(expr, base, "default")
}

fn matches_member(expr: &Expr, base: &Ident, prop_name: &str) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    let Expr::Ident(obj_ident) = member.obj.as_ref() else {
        return false;
    };
    if !ident_matches_binding(obj_ident, &binding_key(base)) {
        return false;
    }
    match &member.prop {
        MemberProp::Ident(prop) => prop.sym.as_ref() == prop_name,
        MemberProp::Computed(prop) => {
            matches!(prop.expr.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(prop_name))
        }
        _ => false,
    }
}

fn should_remove_decl(decl: &VarDeclarator, to_inline: &HashMap<BindingKey, Ident>) -> bool {
    var_declarator_binding_key(decl).is_some_and(|key| to_inline.contains_key(&key))
}

fn build_shadow_avoidance_renames(
    module: &Module,
    to_inline: &mut HashMap<BindingKey, Ident>,
) -> Vec<BindingRename> {
    let mut used_names = collect_declared_names(module);
    let mut base_renames: HashMap<BindingKey, Atom> = HashMap::new();

    for (getter, replacement) in to_inline.iter_mut() {
        if !binding_replacement_would_be_shadowed(module, getter, &replacement.sym) {
            continue;
        }

        let base_key = binding_key(replacement);
        let new_name = base_renames
            .entry(base_key)
            .or_insert_with(|| fresh_prefixed_name(&replacement.sym, &mut used_names))
            .clone();
        replacement.sym = new_name;
    }

    base_renames
        .into_iter()
        .map(|(old, new)| BindingRename { old, new })
        .collect()
}

fn collect_declared_names(module: &Module) -> HashSet<Atom> {
    struct Collector {
        names: HashSet<Atom>,
    }

    impl Visit for Collector {
        fn visit_pat(&mut self, pat: &Pat) {
            collect_pat_names(pat, &mut self.names);
        }
    }

    let mut names = collect_module_names(module);
    let mut collector = Collector {
        names: HashSet::new(),
    };
    module.visit_with(&mut collector);
    names.extend(collector.names);
    names
}

fn fresh_prefixed_name(name: &Atom, used_names: &mut HashSet<Atom>) -> Atom {
    let base = format!("_{name}");
    let atom = Atom::from(base);
    if used_names.insert(atom.clone()) {
        return atom;
    }

    let mut index = 2usize;
    loop {
        let candidate = Atom::from(format!("_{name}{index}"));
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

struct GetterUsageCollector<'a> {
    usage: &'a mut HashMap<BindingKey, UsageStats>,
}

impl GetterUsageCollector<'_> {
    fn visit_item(&mut self, item: &ModuleItem) {
        match item {
            ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var))) => {
                self.visit_var_decl(var);
            }
            _ => item.visit_with(self),
        }
    }

    fn mark_supported(&mut self, ident: &Ident) {
        if let Some(stats) = self.usage.get_mut(&binding_key(ident)) {
            stats.supported += 1;
        }
    }

    fn mark_unsupported(&mut self, ident: &Ident) {
        if let Some(stats) = self.usage.get_mut(&binding_key(ident)) {
            stats.unsupported = true;
        }
    }
}

impl Visit for GetterUsageCollector<'_> {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        for decl in &var.decls {
            if let Some(init) = &decl.init {
                init.visit_with(self);
            }
        }
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(id) = callee.as_ref() {
                if self.usage.contains_key(&binding_key(id)) {
                    if call.args.is_empty() {
                        self.mark_supported(id);
                    } else {
                        self.mark_unsupported(id);
                    }
                    for arg in &call.args {
                        arg.visit_with(self);
                    }
                    return;
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if let Expr::Ident(id) = member.obj.as_ref() {
            if self.usage.contains_key(&binding_key(id)) {
                let is_dot_a = match &member.prop {
                    MemberProp::Ident(prop) => prop.sym.as_ref() == "a",
                    MemberProp::Computed(prop) => {
                        matches!(prop.expr.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("a"))
                    }
                    _ => false,
                };
                if is_dot_a {
                    self.mark_supported(id);
                } else {
                    self.mark_unsupported(id);
                }
                if let MemberProp::Computed(prop) = &member.prop {
                    prop.visit_with(self);
                }
                return;
            }
        }
        member.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.mark_unsupported(ident);
    }

    fn visit_prop_name(&mut self, _: &swc_core::ecma::ast::PropName) {}

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_with(self);
        }
    }
}

struct GetterReplacer<'a> {
    map: &'a HashMap<BindingKey, Ident>,
}

impl VisitMut for GetterReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Call(call) = expr {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(id) = callee.as_ref() {
                    if call.args.is_empty() {
                        if let Some(replacement) = self.map.get(&binding_key(id)) {
                            *expr = Expr::Ident(replacement.clone());
                            return;
                        }
                    }
                }
            }
        }

        if let Expr::Member(member) = expr {
            if let Expr::Ident(id) = member.obj.as_ref() {
                let is_dot_a = match &member.prop {
                    MemberProp::Ident(prop) => prop.sym.as_ref() == "a",
                    MemberProp::Computed(prop) => {
                        matches!(prop.expr.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("a"))
                    }
                    _ => false,
                };
                if is_dot_a {
                    if let Some(replacement) = self.map.get(&binding_key(id)) {
                        *expr = Expr::Ident(replacement.clone());
                    }
                }
            }
        }
    }

    fn visit_mut_prop_name(&mut self, _: &mut swc_core::ecma::ast::PropName) {}

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(prop) = prop {
            prop.visit_mut_with(self);
        }
    }
}

struct WebpackNamespaceReplacer<'a> {
    initial_ref_counts: &'a HashMap<BindingKey, usize>,
    module_bindings: &'a HashSet<BindingKey>,
    removed_caches: HashSet<BindingKey>,
    unresolved_mark: Mark,
}

impl VisitMut for WebpackNamespaceReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Member(member) = expr else {
            return;
        };
        let Some(prop_name) = static_member_prop_name(&member.prop) else {
            return;
        };
        let Some(namespace) = match_require_t_namespace(
            member.obj.as_ref(),
            self.module_bindings,
            self.unresolved_mark,
        ) else {
            return;
        };
        if let Some(cache) = namespace.cache {
            let total_refs = self.initial_ref_counts.get(&cache).copied().unwrap_or(0);
            let local_refs = count_binding_refs_in_expr(member.obj.as_ref(), &cache);
            if total_refs != local_refs {
                return;
            }
            self.removed_caches.insert(cache);
        }

        if prop_name == "default" {
            *expr = Expr::Ident(namespace.base);
        } else {
            *member.obj = Expr::Ident(namespace.base);
        }
    }

    fn visit_mut_prop_name(&mut self, _: &mut swc_core::ecma::ast::PropName) {}
}

struct WebpackHasOwnReplacer {
    unresolved_mark: Mark,
}

impl VisitMut for WebpackHasOwnReplacer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else {
            return;
        };
        if !is_require_o_call(call, self.unresolved_mark) {
            return;
        }

        let mut args = std::mem::take(&mut call.args);
        let property = args.pop().expect("length checked").expr;
        let object = args.pop().expect("length checked").expr;
        *expr = Expr::Call(CallExpr {
            span: call.span,
            ctxt: call.ctxt,
            callee: object_prototype_has_own_property_call_callee(),
            args: vec![
                swc_core::ecma::ast::ExprOrSpread {
                    spread: None,
                    expr: object,
                },
                swc_core::ecma::ast::ExprOrSpread {
                    spread: None,
                    expr: property,
                },
            ],
            type_args: None,
        });
    }
}

fn is_require_o_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    if call.args.len() != 2 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(require_ident) = member.obj.as_ref() else {
        return false;
    };
    require_ident.sym.as_ref() == "require"
        && require_ident.ctxt.outer() == unresolved_mark
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "o")
}

fn object_prototype_has_own_property_call_callee() -> Callee {
    let object = Expr::Ident(Ident::new_no_ctxt("Object".into(), DUMMY_SP));
    let prototype = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(object),
        prop: MemberProp::Ident(IdentName::new("prototype".into(), DUMMY_SP)),
    });
    let has_own_property = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(prototype),
        prop: MemberProp::Ident(IdentName::new("hasOwnProperty".into(), DUMMY_SP)),
    });
    let call = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(has_own_property),
        prop: MemberProp::Ident(IdentName::new("call".into(), DUMMY_SP)),
    });
    Callee::Expr(Box::new(call))
}
