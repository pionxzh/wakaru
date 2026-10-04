use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprStmt, IdentName, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, ObjectLit, Prop, PropName, PropOrSpread,
    SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp,
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
        items.retain(|item| match item {
            ModuleItem::Stmt(stmt) => !is_marker_stmt(stmt, self.unresolved_mark),
            _ => true,
        });
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !is_marker_stmt(stmt, self.unresolved_mark));
    }
}

/// Whether the module marks itself `__esModule` at the top level.
pub(crate) fn has_top_level_esmodule_flag(module: &Module, unresolved_mark: Mark) -> bool {
    module
        .body
        .iter()
        .any(|item| is_esmodule_item(item, unresolved_mark))
}

fn is_esmodule_item(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    match item {
        ModuleItem::Stmt(stmt) => is_esmodule_stmt(stmt, unresolved_mark),
        _ => false,
    }
}

/// An interop marker statement: the `__esModule` flag, or rollup's
/// `Symbol.toStringTag` marker on its own. Only the first counts as evidence
/// that the module was compiled from ESM (see [`has_top_level_esmodule_flag`]).
fn is_marker_stmt(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    if is_esmodule_stmt(stmt, unresolved_mark) {
        return true;
    }
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    matches!(&**expr, Expr::Call(call) if is_to_string_tag_define_property_call(call, unresolved_mark))
}

fn is_esmodule_stmt(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    match &**expr {
        Expr::Call(call) => {
            is_define_property_call(call, unresolved_mark)
                || is_webpack_require_r_call(call, unresolved_mark)
                || is_marker_define_properties_call(call, unresolved_mark)
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

/// The callee `Object.<method>` with the global `Object`.
fn is_object_method_call(call: &CallExpr, method: &str, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    matches!(&**obj, Expr::Ident(id) if &*id.sym == "Object" && id.ctxt.outer() == unresolved_mark)
        && matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == method)
}

/// `Symbol.toStringTag` with the global `Symbol`.
fn is_to_string_tag(expr: &Expr, unresolved_mark: Mark) -> bool {
    matches!(expr, Expr::Member(MemberExpr { obj, prop, .. })
        if matches!(&**obj, Expr::Ident(id) if &*id.sym == "Symbol" && id.ctxt.outer() == unresolved_mark)
            && matches!(prop, MemberProp::Ident(IdentName { sym, .. }) if &**sym == "toStringTag"))
}

/// The descriptor `{ value: <expected> }` with no other property.
fn is_value_descriptor(expr: &Expr, expected: impl Fn(&Expr) -> bool) -> bool {
    let Expr::Object(ObjectLit { props, .. }) = expr else {
        return false;
    };
    let [PropOrSpread::Prop(prop)] = props.as_slice() else {
        return false;
    };
    matches!(&**prop, Prop::KeyValue(kv)
        if matches!(&kv.key, PropName::Ident(key) if &*key.sym == "value")
            && expected(&kv.value))
}

fn is_module_tag_value(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(s)) if &*s.value == "Module")
}

/// rollup's `Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' })`.
fn is_to_string_tag_define_property_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    is_object_method_call(call, "defineProperty", unresolved_mark)
        && call.args.len() == 3
        && call.args.iter().all(|arg| arg.spread.is_none())
        && is_export_object(&call.args[0].expr, unresolved_mark)
        && is_to_string_tag(&call.args[1].expr, unresolved_mark)
        && is_value_descriptor(&call.args[2].expr, is_module_tag_value)
}

/// rollup's `Object.defineProperties(exports, { __esModule: { value: true },
/// [Symbol.toStringTag]: { value: 'Module' } })`, with nothing else defined.
fn is_marker_define_properties_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    if !is_object_method_call(call, "defineProperties", unresolved_mark)
        || call.args.len() != 2
        || call.args.iter().any(|arg| arg.spread.is_some())
        || !is_export_object(&call.args[0].expr, unresolved_mark)
    {
        return false;
    }
    let Expr::Object(ObjectLit { props, .. }) = &*call.args[1].expr else {
        return false;
    };
    let mut esmodule = false;
    for prop in props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(kv) = &**prop else {
            return false;
        };
        match &kv.key {
            PropName::Ident(key) if &*key.sym == "__esModule" => {
                if !is_value_descriptor(&kv.value, is_loose_true) {
                    return false;
                }
                esmodule = true;
            }
            PropName::Str(key) if &*key.value == "__esModule" => {
                if !is_value_descriptor(&kv.value, is_loose_true) {
                    return false;
                }
                esmodule = true;
            }
            PropName::Computed(key) if is_to_string_tag(&key.expr, unresolved_mark) => {
                if !is_value_descriptor(&kv.value, is_module_tag_value) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    esmodule
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
