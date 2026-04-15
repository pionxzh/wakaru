use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    BlockStmtOrExpr, Callee, Expr, Ident, IfStmt, Lit, MemberExpr, MemberProp, Module, ModuleItem,
    Pat, ReturnStmt, Stmt, VarDecl, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

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
