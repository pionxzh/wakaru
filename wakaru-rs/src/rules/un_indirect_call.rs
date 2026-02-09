use swc_core::ecma::ast::{CallExpr, Callee, Expr, Lit, SeqExpr};
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
        let Some(seq) = as_seq_expr(callee_expr) else {
            return;
        };
        let exprs = &seq.exprs;
        if exprs.len() != 2 {
            return;
        }
        if !matches!(&*exprs[0], Expr::Lit(Lit::Num(num)) if num.value == 0.0) {
            return;
        }
        let Some(member_expr) = as_member_expr(&exprs[1]) else {
            return;
        };

        *expr = Expr::Call(CallExpr {
            span: *span,
            ctxt: *ctxt,
            callee: Callee::Expr(member_expr),
            args: args.clone(),
            type_args: type_args.clone(),
        });
    }
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

fn as_member_expr(expr: &Expr) -> Option<Box<Expr>> {
    match strip_paren(expr) {
        Expr::Member(_) => Some(Box::new(strip_paren(expr).clone())),
        _ => None,
    }
}
