use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Lit, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct FlipComparisons;

impl VisitMut for FlipComparisons {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Bin(BinExpr { op, left, right, .. }) = expr {
            if is_equality(*op) {
                if is_flippable_literal_like(left) && !is_flippable_literal_like(right) {
                    std::mem::swap(left, right);
                }
                return;
            }

            if is_relational(*op) && is_flippable_literal_like(left) && !is_flippable_literal_like(right) {
                std::mem::swap(left, right);
                *op = flipped_relational(*op);
            }
        }
    }
}

fn is_equality(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::EqEq | BinaryOp::NotEq | BinaryOp::EqEqEq | BinaryOp::NotEqEq
    )
}

fn is_relational(op: BinaryOp) -> bool {
    matches!(op, BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq)
}

fn flipped_relational(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::LtEq => BinaryOp::GtEq,
        BinaryOp::GtEq => BinaryOp::LtEq,
        _ => op,
    }
}

fn is_flippable_literal_like(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(Lit::Null(_))
        | Expr::Lit(Lit::Bool(_))
        | Expr::Lit(Lit::Num(_))
        | Expr::Lit(Lit::Str(_))
        | Expr::Lit(Lit::BigInt(_)) => true,
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        Expr::Ident(ident) => ident.sym == "undefined" || ident.sym == "NaN" || ident.sym == "Infinity",
        Expr::Unary(UnaryExpr { op: UnaryOp::Void, arg, .. }) => {
            matches!(&**arg, Expr::Lit(Lit::Num(_)))
        }
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Minus,
            arg,
            ..
        }) => matches!(&**arg, Expr::Ident(ident) if ident.sym == "Infinity"),
        _ => false,
    }
}
