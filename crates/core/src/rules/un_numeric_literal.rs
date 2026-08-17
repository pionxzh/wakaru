use swc_core::ecma::ast::{Expr, Lit};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnNumericLiteral;

impl VisitMut for UnNumericLiteral {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Lit(Lit::Num(num)) = expr {
            // SWC stores overflowing literals such as `1e999` as infinity.
            // Dropping their raw spelling asks codegen to invent a finite
            // approximation (currently `2e308`), which is not a readability
            // normalization and is unstable across printer versions.
            if !num.value.is_finite() {
                return;
            }
            if let Some(raw) = &num.raw {
                if raw.as_ref() != num.value.to_string() {
                    num.raw = None;
                }
            }
        }
    }
}
