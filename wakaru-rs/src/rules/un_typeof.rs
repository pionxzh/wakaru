use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Lit, Str, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnTypeof;

impl VisitMut for UnTypeof {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Bin(BinExpr {
            op,
            left,
            right,
            span,
        }) = expr
        {
            if is_typeof(left) && is_u_string(right) {
                let next_op = match *op {
                    BinaryOp::Lt => Some(BinaryOp::NotEqEq),
                    BinaryOp::Gt => Some(BinaryOp::EqEqEq),
                    _ => None,
                };
                if let Some(next_op) = next_op {
                    *expr = Expr::Bin(BinExpr {
                        span: *span,
                        op: next_op,
                        left: left.clone(),
                        right: undefined_string(*span),
                    });
                }
                return;
            }

            if is_u_string(left) && is_typeof(right) {
                let next_op = match *op {
                    BinaryOp::Lt => Some(BinaryOp::EqEqEq),
                    BinaryOp::Gt => Some(BinaryOp::NotEqEq),
                    _ => None,
                };
                if let Some(next_op) = next_op {
                    *expr = Expr::Bin(BinExpr {
                        span: *span,
                        op: next_op,
                        left: right.clone(),
                        right: undefined_string(*span),
                    });
                }
            }
        }
    }
}

fn is_typeof(expr: &Expr) -> bool {
    matches!(expr, Expr::Unary(UnaryExpr { op: UnaryOp::TypeOf, .. }))
}

fn is_u_string(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(Str { value, .. })) if value == "u")
}

fn undefined_string(span: swc_core::common::Span) -> Box<Expr> {
    Box::new(Expr::Lit(Lit::Str(Str {
        span,
        value: "undefined".into(),
        raw: None,
    })))
}
