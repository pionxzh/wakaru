use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, Callee, Decl, Expr, Function, Ident, IfStmt, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, ObjectPatProp, Pat, PropName, ReturnStmt, Stmt,
    VarDecl, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::rename_utils::{collect_module_names, rename_bindings_in_module, BindingRename};

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
pub struct UnWebpackInterop;

type BindingKey = (Atom, SyntaxContext);

#[derive(Default)]
struct UsageStats {
    supported: usize,
    unsupported: bool,
}

impl VisitMut for UnWebpackInterop {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let require_bindings = collect_require_bindings(module);
        if require_bindings.is_empty() {
            return;
        }

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
                if let Some(base) = match_interop_getter(init.as_ref(), &require_bindings) {
                    candidates.insert((binding.id.sym.clone(), binding.id.ctxt), base);
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

fn collect_require_bindings(module: &Module) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(swc_core::ecma::ast::Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let Some(init) = &decl.init else {
                        continue;
                    };
                    if is_require_call(init.as_ref()) {
                        bindings.insert((binding.id.sym.clone(), binding.id.ctxt));
                    }
                }
            }
            _ => {}
        }
    }
    bindings
}

fn is_require_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require")
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
    let base_key = (base.sym.clone(), base.ctxt);
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
    if alt_ident.sym != base.sym || alt_ident.ctxt != base.ctxt {
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
    let base_key = (base.sym.clone(), base.ctxt);
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
    if alt_ident.sym != base.sym || alt_ident.ctxt != base.ctxt {
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
    if obj_ident.sym != base.sym || obj_ident.ctxt != base.ctxt {
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
    let Pat::Ident(binding) = &decl.name else {
        return false;
    };
    to_inline.contains_key(&(binding.id.sym.clone(), binding.id.ctxt))
}

fn build_shadow_avoidance_renames(
    module: &Module,
    to_inline: &mut HashMap<BindingKey, Ident>,
) -> Vec<BindingRename> {
    let mut used_names = collect_declared_names(module);
    let mut base_renames: HashMap<BindingKey, Atom> = HashMap::new();

    for (getter, replacement) in to_inline.iter_mut() {
        if !getter_replacement_would_be_shadowed(module, getter, &replacement.sym) {
            continue;
        }

        let base_key = (replacement.sym.clone(), replacement.ctxt);
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

fn getter_replacement_would_be_shadowed(
    module: &Module,
    getter: &BindingKey,
    replacement_name: &Atom,
) -> bool {
    struct Checker<'a> {
        getter: &'a BindingKey,
        replacement_name: &'a Atom,
        scope_stack: Vec<bool>,
        found: bool,
    }

    impl Checker<'_> {
        fn in_shadowing_scope(&self) -> bool {
            self.scope_stack.iter().any(|declares| *declares)
        }

        fn pat_binds_replacement(&self, pat: &Pat) -> bool {
            pat_binds_name(pat, self.replacement_name)
        }

        fn mark_scope_decl(&mut self, pat: &Pat) {
            if !self.scope_stack.is_empty() && self.pat_binds_replacement(pat) {
                if let Some(scope) = self.scope_stack.last_mut() {
                    *scope = true;
                }
            }
        }

        fn is_getter_ident(&self, ident: &Ident) -> bool {
            ident.sym == self.getter.0 && ident.ctxt == self.getter.1
        }
    }

    impl Visit for Checker<'_> {
        fn visit_function(&mut self, function: &Function) {
            let params_shadow = function
                .params
                .iter()
                .any(|param| self.pat_binds_replacement(&param.pat));
            let body_shadow = function
                .body
                .as_ref()
                .is_some_and(|body| scope_body_binds_name(body, self.replacement_name));
            self.scope_stack.push(params_shadow || body_shadow);
            function.visit_children_with(self);
            self.scope_stack.pop();
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            let params_shadow = arrow
                .params
                .iter()
                .any(|param| self.pat_binds_replacement(param));
            let body_shadow = match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(body) => {
                    scope_body_binds_name(body, self.replacement_name)
                }
                BlockStmtOrExpr::Expr(_) => false,
            };
            self.scope_stack.push(params_shadow || body_shadow);
            arrow.visit_children_with(self);
            self.scope_stack.pop();
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            self.mark_scope_decl(&declarator.name);
            declarator.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(id) = callee.as_ref() {
                    if call.args.is_empty() && self.is_getter_ident(id) {
                        if self.in_shadowing_scope() {
                            self.found = true;
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
                if self.is_getter_ident(id) && is_dot_a(&member.prop) {
                    if self.in_shadowing_scope() {
                        self.found = true;
                    }
                    if let MemberProp::Computed(prop) = &member.prop {
                        prop.visit_with(self);
                    }
                    return;
                }
            }
            member.visit_children_with(self);
        }

        fn visit_prop_name(&mut self, _: &PropName) {}

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(prop) = prop {
                prop.visit_with(self);
            }
        }
    }

    let mut checker = Checker {
        getter,
        replacement_name,
        scope_stack: Vec::new(),
        found: false,
    };
    module.visit_with(&mut checker);
    checker.found
}

fn scope_body_binds_name(body: &BlockStmt, name: &Atom) -> bool {
    struct Collector<'a> {
        name: &'a Atom,
        found: bool,
    }

    impl Collector<'_> {
        fn pat_binds_name(&self, pat: &Pat) -> bool {
            pat_binds_name(pat, self.name)
        }
    }

    impl Visit for Collector<'_> {
        fn visit_decl(&mut self, decl: &Decl) {
            match decl {
                Decl::Var(var) => {
                    if var.decls.iter().any(|decl| self.pat_binds_name(&decl.name)) {
                        self.found = true;
                    }
                    for decl in &var.decls {
                        if let Some(init) = &decl.init {
                            init.visit_with(self);
                        }
                    }
                }
                Decl::Fn(function) => {
                    if &function.ident.sym == self.name {
                        self.found = true;
                    }
                }
                Decl::Class(class) => {
                    if &class.ident.sym == self.name {
                        self.found = true;
                    }
                    class.class.visit_children_with(self);
                }
                _ => decl.visit_children_with(self),
            }
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

        fn visit_prop_name(&mut self, _: &PropName) {}

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(prop) = prop {
                prop.visit_with(self);
            }
        }
    }

    let mut collector = Collector { name, found: false };
    body.visit_with(&mut collector);
    collector.found
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

fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(id) => {
            names.insert(id.id.sym.clone());
        }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_names(elem, names);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_names(&kv.value, names),
                    ObjectPatProp::Assign(assign) => {
                        names.insert(assign.key.id.sym.clone());
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_names(&rest.arg, names),
                }
            }
        }
        Pat::Assign(assign) => collect_pat_names(&assign.left, names),
        Pat::Rest(rest) => collect_pat_names(&rest.arg, names),
        _ => {}
    }
}

fn pat_binds_name(pat: &Pat, name: &Atom) -> bool {
    match pat {
        Pat::Ident(id) => &id.id.sym == name,
        Pat::Array(arr) => arr.elems.iter().flatten().any(|p| pat_binds_name(p, name)),
        Pat::Object(obj) => obj.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => pat_binds_name(&kv.value, name),
            ObjectPatProp::Assign(assign) => &assign.key.id.sym == name,
            ObjectPatProp::Rest(rest) => pat_binds_name(&rest.arg, name),
        }),
        Pat::Assign(assign) => pat_binds_name(&assign.left, name),
        Pat::Rest(rest) => pat_binds_name(&rest.arg, name),
        _ => false,
    }
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

fn is_dot_a(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(prop) => prop.sym.as_ref() == "a",
        MemberProp::Computed(prop) => {
            matches!(prop.expr.as_ref(), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some("a"))
        }
        _ => false,
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
        if let Some(stats) = self.usage.get_mut(&(ident.sym.clone(), ident.ctxt)) {
            stats.supported += 1;
        }
    }

    fn mark_unsupported(&mut self, ident: &Ident) {
        if let Some(stats) = self.usage.get_mut(&(ident.sym.clone(), ident.ctxt)) {
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
                if self.usage.contains_key(&(id.sym.clone(), id.ctxt)) {
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
            if self.usage.contains_key(&(id.sym.clone(), id.ctxt)) {
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
                        if let Some(replacement) = self.map.get(&(id.sym.clone(), id.ctxt)) {
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
                    if let Some(replacement) = self.map.get(&(id.sym.clone(), id.ctxt)) {
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
