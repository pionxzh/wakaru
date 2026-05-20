use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprStmt, IdentName, Lit,
    MemberExpr, MemberProp, ModuleItem, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnEsmoduleFlag {
    unresolved_mark: Mark,
}

impl UnEsmoduleFlag {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnEsmoduleFlag {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| !is_esmodule_item(item, self.unresolved_mark));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !is_esmodule_stmt(stmt, self.unresolved_mark));
    }
}

fn is_esmodule_item(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    match item {
        ModuleItem::Stmt(stmt) => is_esmodule_stmt(stmt, unresolved_mark),
        _ => false,
    }
}

fn is_esmodule_stmt(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    match &**expr {
        Expr::Call(call) => {
            is_define_property_call(call, unresolved_mark)
                || is_webpack_require_r_call(call, unresolved_mark)
        }
        Expr::Assign(assign) => is_esmodule_assign(assign, unresolved_mark),
        _ => false,
    }
}

/// Checks for `Object.defineProperty(exports, '__esModule', { value: true })`
/// or `Object.defineProperty(module.exports, '__esModule', { value: true })`
fn is_define_property_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    // Must be a member call: Object.defineProperty
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !matches!(
        &**obj,
        Expr::Ident(id) if &*id.sym == "Object" && id.ctxt.outer() == unresolved_mark
    ) {
        return false;
    }
    if !matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == "defineProperty") {
        return false;
    }

    // Must have 3 arguments
    if call.args.len() != 3 {
        return false;
    }

    // First arg: exports or module.exports
    if !is_export_object(&call.args[0].expr, unresolved_mark) {
        return false;
    }

    // Second arg: '__esModule'
    if !matches!(&*call.args[1].expr, Expr::Lit(Lit::Str(s)) if &*s.value == "__esModule") {
        return false;
    }

    // Third arg: { value: true } — we accept any object literal with a truthy value property
    // We do a permissive check: just confirm the call pattern is correct (2nd arg is __esModule)
    // and trust that it's the interop flag descriptor
    true
}

/// Checks for webpack's `require.r(exports)` helper, which marks the target as an ES module.
fn is_webpack_require_r_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !matches!(
        &**obj,
        Expr::Ident(id) if &*id.sym == "require" && id.ctxt.outer() == unresolved_mark
    ) {
        return false;
    }
    if !matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == "r") {
        return false;
    }
    call.args.len() == 1 && is_export_object(&call.args[0].expr, unresolved_mark)
}

/// Checks for `exports.__esModule = true/!0` or `module.exports.__esModule = true/!0`
fn is_esmodule_assign(assign: &AssignExpr, unresolved_mark: Mark) -> bool {
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &assign.left else {
        return false;
    };
    if !matches!(&m.prop, MemberProp::Ident(i) if &*i.sym == "__esModule") {
        return false;
    }
    if !is_export_object(&m.obj, unresolved_mark) {
        return false;
    }
    is_loose_true(&assign.right)
}

/// Returns true for `exports` (identifier) or `module.exports` (member expression)
fn is_export_object(expr: &Expr, unresolved_mark: Mark) -> bool {
    if matches!(
        expr,
        Expr::Ident(id) if &*id.sym == "exports" && id.ctxt.outer() == unresolved_mark
    ) {
        return true;
    }
    // module.exports
    if let Expr::Member(MemberExpr { obj, prop, .. }) = expr {
        if matches!(
            &**obj,
            Expr::Ident(id) if &*id.sym == "module" && id.ctxt.outer() == unresolved_mark
        ) && matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == "exports")
        {
            return true;
        }
    }
    false
}

/// Returns true for `true`, `!0`, or `1`
fn is_loose_true(expr: &Expr) -> bool {
    if matches!(expr, Expr::Lit(Lit::Bool(b)) if b.value) {
        return true;
    }
    if matches!(expr, Expr::Lit(Lit::Num(n)) if n.value == 1.0) {
        return true;
    }
    // !0
    if let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = expr
    {
        if matches!(&**arg, Expr::Lit(Lit::Num(n)) if n.value == 0.0) {
            return true;
        }
    }
    false
}
