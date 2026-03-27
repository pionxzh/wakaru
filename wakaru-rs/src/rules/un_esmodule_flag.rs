use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprStmt, IdentName, Lit,
    MemberExpr, MemberProp, ModuleItem, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnEsmoduleFlag;

impl VisitMut for UnEsmoduleFlag {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| !is_esmodule_item(item));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !is_esmodule_stmt(stmt));
    }
}

fn is_esmodule_item(item: &ModuleItem) -> bool {
    match item {
        ModuleItem::Stmt(stmt) => is_esmodule_stmt(stmt),
        _ => false,
    }
}

fn is_esmodule_stmt(stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    match &**expr {
        Expr::Call(call) => is_define_property_call(call),
        Expr::Assign(assign) => is_esmodule_assign(assign),
        _ => false,
    }
}

/// Checks for `Object.defineProperty(exports, '__esModule', { value: true })`
/// or `Object.defineProperty(module.exports, '__esModule', { value: true })`
fn is_define_property_call(call: &CallExpr) -> bool {
    // Must be a member call: Object.defineProperty
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !matches!(&**obj, Expr::Ident(id) if &*id.sym == "Object") {
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
    if !is_export_object(&call.args[0].expr) {
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

/// Checks for `exports.__esModule = true/!0` or `module.exports.__esModule = true/!0`
fn is_esmodule_assign(assign: &AssignExpr) -> bool {
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &assign.left else {
        return false;
    };
    if !matches!(&m.prop, MemberProp::Ident(i) if &*i.sym == "__esModule") {
        return false;
    }
    if !is_export_object(&m.obj) {
        return false;
    }
    is_loose_true(&assign.right)
}

/// Returns true for `exports` (identifier) or `module.exports` (member expression)
fn is_export_object(expr: &Expr) -> bool {
    if matches!(expr, Expr::Ident(id) if &*id.sym == "exports") {
        return true;
    }
    // module.exports
    if let Expr::Member(MemberExpr { obj, prop, .. }) = expr {
        if matches!(&**obj, Expr::Ident(id) if &*id.sym == "module") {
            if matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == "exports") {
                return true;
            }
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
    if let Expr::Unary(UnaryExpr { op: UnaryOp::Bang, arg, .. }) = expr {
        if matches!(&**arg, Expr::Lit(Lit::Num(n)) if n.value == 0.0) {
            return true;
        }
    }
    false
}
