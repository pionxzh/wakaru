use swc_core::ecma::ast::{Bool, Expr, Lit, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnminifyBooleans;

impl VisitMut for UnminifyBooleans {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Unary(UnaryExpr { op, arg, span }) = expr {
            if *op != UnaryOp::Bang {
                return;
            }

            let value = match &**arg {
                Expr::Lit(Lit::Num(num)) if num.value == 0.0 => Some(true),
                Expr::Lit(Lit::Num(num)) if num.value == 1.0 => Some(false),
                _ => None,
            };

            if let Some(value) = value {
                *expr = Expr::Lit(Lit::Bool(Bool { span: *span, value }));
            }
        }
    }
}
