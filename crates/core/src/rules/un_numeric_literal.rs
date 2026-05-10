use swc_core::ecma::ast::{Expr, Lit};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnNumericLiteral;

impl VisitMut for UnNumericLiteral {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Lit(Lit::Num(num)) = expr {
            if let Some(raw) = &num.raw {
                if raw.as_ref() != num.value.to_string() {
                    num.raw = None;
                }
            }
        }
    }
}
