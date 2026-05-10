use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Ident, Lit, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnInfinity;

impl VisitMut for UnInfinity {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Bin(BinExpr {
            op: BinaryOp::Div,
            left,
            right,
            span,
        }) = expr
        {
            if !matches!(&**right, Expr::Lit(Lit::Num(num)) if num.value == 0.0) {
                return;
            }

            if matches!(&**left, Expr::Lit(Lit::Num(num)) if num.value == 1.0) {
                *expr = Expr::Ident(Ident::new_no_ctxt("Infinity".into(), *span));
                return;
            }

            if matches!(&**left, Expr::Unary(UnaryExpr { op: UnaryOp::Minus, arg, .. }) if matches!(&**arg, Expr::Lit(Lit::Num(num)) if num.value == 1.0))
            {
                *expr = Expr::Unary(UnaryExpr {
                    span: *span,
                    op: UnaryOp::Minus,
                    arg: Box::new(Expr::Ident(Ident::new_no_ctxt("Infinity".into(), *span))),
                });
            }
        }
    }
}
