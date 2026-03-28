use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ExprOrSpread, Ident, Lit, MemberProp, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnArgumentSpread;

impl VisitMut for UnArgumentSpread {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

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

        match try_convert_apply(call) {
            Ok(new_expr) => *expr = new_expr,
            Err(original_call) => *expr = Expr::Call(original_call),
        }
    }
}

fn try_convert_apply(call: CallExpr) -> Result<Expr, CallExpr> {
    // callee must be a member expression ending in `.apply`
    let callee_member = match &call.callee {
        Callee::Expr(e) => match e.as_ref() {
            Expr::Member(m) => m,
            _ => return Err(call),
        },
        _ => return Err(call),
    };

    // Check that the property is `apply`
    match &callee_member.prop {
        MemberProp::Ident(ident_name) if ident_name.sym.as_ref() == "apply" => {}
        _ => return Err(call),
    }

    // We need exactly 2 arguments
    if call.args.len() != 2 {
        return Err(call);
    }

    // Check for spread on either arg – we don't handle those
    if call.args[0].spread.is_some() || call.args[1].spread.is_some() {
        return Err(call);
    }

    let first_arg = call.args[0].expr.as_ref();
    let callee_obj = callee_member.obj.as_ref();

    // Pattern 1: fn.apply(null/undefined, arg2) → fn(...arg2)
    // Only applies when the callee object is NOT itself a member expression
    // (i.e., the callee is just `fn`, not `obj.fn`)
    // Actually per the JS spec, for plain fn.apply(null/undefined) we convert regardless.
    // But if it's obj.fn.apply(obj, ...) we match pattern 2 instead.
    // Determine which pattern applies:

    // Pattern 2: obj.fn.apply(obj, arg2) → obj.fn(...arg2)
    // The callee's object is a member expression AND first arg equals the outer object.
    // e.g. callee = obj.fn.apply, callee_obj = obj.fn (Member), first_arg should = obj
    if let Expr::Member(callee_member_obj) = callee_obj {
        if exprs_equal(first_arg, &callee_member_obj.obj) {
            return Ok(make_spread_call(call));
        }
        // Otherwise don't convert (obj.fn.apply(someOtherThing, ...) is not safe)
        return Err(call);
    }

    // Pattern 1: callee obj is not a member expression, first arg must be null/undefined
    if is_null_or_undefined(first_arg) {
        return Ok(make_spread_call(call));
    }

    Err(call)
}

/// Build `fn(...secondArg)` from the original `.apply(thisArg, secondArg)` call.
fn make_spread_call(call: CallExpr) -> Expr {
    // Consume the call
    let CallExpr {
        span,
        ctxt,
        callee,
        mut args,
        type_args,
    } = call;

    // callee is `fn.apply` – we want just `fn`
    let Callee::Expr(callee_box) = callee else {
        unreachable!()
    };
    let Expr::Member(member) = *callee_box else {
        unreachable!()
    };
    let fn_expr = member.obj;

    // second arg becomes the spread argument
    let second_arg = args.remove(1).expr;

    Expr::Call(CallExpr {
        span,
        ctxt,
        callee: Callee::Expr(fn_expr),
        args: vec![ExprOrSpread {
            spread: Some(DUMMY_SP),
            expr: second_arg,
        }],
        type_args,
    })
}

fn is_null_or_undefined(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Ident(Ident { sym, .. }) if sym.as_ref() == "undefined" => true,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Void,
            arg,
            ..
        }) => matches!(
            arg.as_ref(),
            Expr::Lit(Lit::Num(n)) if n.value == 0.0
        ),
        _ => false,
    }
}

fn exprs_equal(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Ident(ai), Expr::Ident(bi)) => ai.sym == bi.sym,
        (Expr::This(_), Expr::This(_)) => true,
        (Expr::Member(am), Expr::Member(bm)) => {
            exprs_equal(&am.obj, &bm.obj) && member_props_equal(&am.prop, &bm.prop)
        }
        (Expr::Array(aa), Expr::Array(ab)) => {
            // Two array literals are equal only if both are empty
            aa.elems.is_empty() && ab.elems.is_empty()
        }
        (Expr::Lit(la), Expr::Lit(lb)) => lits_equal(la, lb),
        _ => false,
    }
}

fn member_props_equal(a: &MemberProp, b: &MemberProp) -> bool {
    match (a, b) {
        (MemberProp::Ident(ai), MemberProp::Ident(bi)) => ai.sym == bi.sym,
        (MemberProp::Computed(ac), MemberProp::Computed(bc)) => exprs_equal(&ac.expr, &bc.expr),
        _ => false,
    }
}

fn lits_equal(a: &Lit, b: &Lit) -> bool {
    match (a, b) {
        (Lit::Str(as_), Lit::Str(bs)) => as_.value == bs.value,
        (Lit::Num(an), Lit::Num(bn)) => an.value == bn.value,
        (Lit::Bool(ab), Lit::Bool(bb)) => ab.value == bb.value,
        (Lit::Null(_), Lit::Null(_)) => true,
        _ => false,
    }
}
