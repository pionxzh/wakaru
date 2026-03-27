use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, Callee, Expr, FnExpr, Ident, IdentName, Lit, MemberExpr, MemberProp,
    Regex,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnBuiltinPrototype;

impl VisitMut for UnBuiltinPrototype {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        // We need an owned value to work with, so take and replace.
        let taken = match expr {
            Expr::Call(_) => {
                let placeholder = Expr::Lit(Lit::Num(swc_core::ecma::ast::Number {
                    span: DUMMY_SP,
                    value: 0.0,
                    raw: None,
                }));
                std::mem::replace(expr, placeholder)
            }
            _ => return,
        };

        let Expr::Call(call) = taken else {
            *expr = taken;
            return;
        };

        match try_replace_builtin(call) {
            Ok(new_expr) => *expr = new_expr,
            Err(original_call) => *expr = Expr::Call(original_call),
        }
    }
}

/// Try to rewrite `instance.method.call(...)` → `BuiltIn.prototype.method.call(...)`
/// Returns Ok(new_expr) on success, Err(original_call) on failure.
fn try_replace_builtin(call: CallExpr) -> Result<Expr, CallExpr> {
    // callee must be a member expression: `instance.method.call` or `instance.method.apply`
    let callee_expr = match &call.callee {
        Callee::Expr(e) => e.as_ref(),
        _ => return Err(call),
    };

    let outer_member = match callee_expr {
        Expr::Member(m) => m,
        _ => return Err(call),
    };

    // outer_member.prop must be `call` or `apply`
    let outer_prop_name = match &outer_member.prop {
        MemberProp::Ident(ident_name) => ident_name.sym.as_ref(),
        _ => return Err(call),
    };
    if outer_prop_name != "call" && outer_prop_name != "apply" {
        return Err(call);
    }

    // outer_member.obj must itself be a member expression: `instance.method`
    let inner_member = match outer_member.obj.as_ref() {
        Expr::Member(m) => m,
        _ => return Err(call),
    };

    // inner_member.obj is the instance literal (may be wrapped in parens)
    let builtin_name = match detect_builtin(strip_paren(inner_member.obj.as_ref())) {
        Some(name) => name,
        None => return Err(call),
    };

    // Now rebuild: BuiltIn.prototype.method.call_or_apply(args)
    // Consume call
    let Callee::Expr(callee_box) = call.callee else {
        unreachable!()
    };
    let Expr::Member(outer_member_owned) = *callee_box else {
        unreachable!()
    };
    let outer_prop = outer_member_owned.prop; // "call" or "apply"
    let Expr::Member(inner_member_owned) = *outer_member_owned.obj else {
        unreachable!()
    };
    let method_prop = inner_member_owned.prop; // the actual method name

    // Build `BuiltIn.prototype`
    let builtin_prototype = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(Ident::new_no_ctxt(builtin_name.into(), DUMMY_SP))),
        prop: MemberProp::Ident(IdentName::new("prototype".into(), DUMMY_SP)),
    });

    // Build `BuiltIn.prototype.method`
    let prototype_method = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(builtin_prototype),
        prop: method_prop,
    });

    // Build `BuiltIn.prototype.method.call` (or `.apply`)
    let new_callee = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(prototype_method),
        prop: outer_prop,
    });

    Ok(Expr::Call(CallExpr {
        span: call.span,
        ctxt: call.ctxt,
        callee: Callee::Expr(Box::new(new_callee)),
        args: call.args,
        type_args: call.type_args,
    }))
}

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => strip_paren(&p.expr),
        _ => expr,
    }
}

fn detect_builtin(obj: &Expr) -> Option<&'static str> {
    match obj {
        Expr::Array(a) if a.elems.is_empty() => Some("Array"),
        Expr::Lit(Lit::Num(n)) if n.value == 0.0 => Some("Number"),
        Expr::Object(o) if o.props.is_empty() => Some("Object"),
        Expr::Lit(Lit::Regex(Regex { exp, .. })) if !exp.is_empty() => Some("RegExp"),
        Expr::Lit(Lit::Str(s)) if s.value.is_empty() => Some("String"),
        Expr::Fn(FnExpr { .. }) | Expr::Arrow(ArrowExpr { .. }) => Some("Function"),
        _ => None,
    }
}
