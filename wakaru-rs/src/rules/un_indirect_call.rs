use swc_core::ecma::ast::{CallExpr, Callee, Expr, Ident, Lit, SeqExpr};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnIndirectCall;

impl VisitMut for UnIndirectCall {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(CallExpr {
            span,
            ctxt,
            callee,
            args,
            type_args,
        }) = expr
        else {
            return;
        };

        let Callee::Expr(callee_expr) = callee else {
            return;
        };

        // Pattern 1: (0, fn.method)(args) → fn.method(args)
        if let Some(seq) = as_seq_expr(callee_expr) {
            let exprs = &seq.exprs;
            if exprs.len() == 2
                && matches!(&*exprs[0], Expr::Lit(Lit::Num(num)) if num.value == 0.0)
            {
                if let Some(inner) = as_member_or_ident(&exprs[1]) {
                    *expr = Expr::Call(CallExpr {
                        span: *span,
                        ctxt: *ctxt,
                        callee: Callee::Expr(inner),
                        args: args.clone(),
                        type_args: type_args.clone(),
                    });
                }
            }
            return;
        }

        // Pattern 2: Object(fn.method)(args) → fn.method(args)
        // Object() called on a function just returns it — used as indirect call
        if let Some(inner) = as_object_wrap_call(callee_expr) {
            *expr = Expr::Call(CallExpr {
                span: *span,
                ctxt: *ctxt,
                callee: Callee::Expr(inner),
                args: args.clone(),
                type_args: type_args.clone(),
            });
        }
    }
}

/// If `expr` is `Object(inner)` where inner is a member or ident expr, return `inner`.
fn as_object_wrap_call(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Call(call) = strip_paren(expr) else {
        return None;
    };
    if !call.args.is_empty() && call.args.len() != 1 {
        return None;
    }
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    // Must be exactly `Object`
    let Expr::Ident(Ident { sym, .. }) = strip_paren(callee_expr) else {
        return None;
    };
    if sym.as_str() != "Object" {
        return None;
    }
    let arg = call.args.first()?;
    if arg.spread.is_some() {
        return None;
    }
    as_member_or_ident(&arg.expr)
}

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren(&paren.expr),
        _ => expr,
    }
}

fn as_seq_expr(expr: &Expr) -> Option<&SeqExpr> {
    match strip_paren(expr) {
        Expr::Seq(seq) => Some(seq),
        _ => None,
    }
}

fn as_member_or_ident(expr: &Expr) -> Option<Box<Expr>> {
    match strip_paren(expr) {
        Expr::Member(_) | Expr::Ident(_) => Some(Box::new(strip_paren(expr).clone())),
        _ => None,
    }
}
