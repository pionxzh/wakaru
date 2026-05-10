use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Lit, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Upgrade `typeof x == "string"` to `typeof x === "string"`.
///
/// `typeof` always returns a string, so loose and strict equality are
/// equivalent when comparing against a string literal. Every style guide
/// and linter prefers `===`.
pub struct UnTypeofStrict;

impl VisitMut for UnTypeofStrict {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Bin(bin) = expr else { return };

        let upgrade = match bin.op {
            BinaryOp::EqEq => BinaryOp::EqEqEq,
            BinaryOp::NotEq => BinaryOp::NotEqEq,
            _ => return,
        };

        if is_typeof_vs_string(bin) {
            bin.op = upgrade;
        }
    }
}

/// Check if one side is `typeof expr` and the other is a string literal.
fn is_typeof_vs_string(bin: &BinExpr) -> bool {
    (is_typeof(&bin.left) && is_string_lit(&bin.right))
        || (is_typeof(&bin.right) && is_string_lit(&bin.left))
}

fn is_typeof(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::TypeOf,
            ..
        })
    )
}

fn is_string_lit(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(_)))
}
