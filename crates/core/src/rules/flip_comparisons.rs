use swc_core::common::Mark;
use swc_core::ecma::ast::{BinExpr, BinaryOp, Expr, Lit, Module, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct FlipComparisons {
    unresolved_mark: Mark,
    /// The module contains a `with` statement or a direct eval.
    dynamic_scope: bool,
}

impl FlipComparisons {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self {
            unresolved_mark,
            dynamic_scope: false,
        }
    }
}

impl VisitMut for FlipComparisons {
    fn visit_mut_module(&mut self, module: &mut Module) {
        self.dynamic_scope = super::eval_utils::has_dynamic_scope_construct(module);
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Bin(BinExpr {
            op, left, right, ..
        }) = expr
        {
            // Swapping reorders the reads of `undefined`, `NaN`, or
            // `Infinity`, which a `with` getter or an eval-declared binding
            // with `valueOf` can observe; leave such comparisons as written
            // (docs/rewrite-assumptions.md, dynamic-scope skip).
            if self.dynamic_scope
                && (is_global_constant_ident(left, self.unresolved_mark)
                    || is_global_constant_ident(right, self.unresolved_mark))
            {
                return;
            }
            if is_equality(*op) {
                if is_flippable_literal_like(left, self.unresolved_mark)
                    && !is_flippable_literal_like(right, self.unresolved_mark)
                {
                    std::mem::swap(left, right);
                }
                return;
            }

            if is_relational(*op)
                && is_flippable_literal_like(left, self.unresolved_mark)
                && !is_flippable_literal_like(right, self.unresolved_mark)
            {
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
    matches!(
        op,
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::LtEq | BinaryOp::GtEq
    )
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

/// `undefined`, `NaN`, `Infinity`, or `-Infinity` read as the global.
fn is_global_constant_ident(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Ident(ident) => {
            matches!(ident.sym.as_ref(), "undefined" | "NaN" | "Infinity")
                && ident.ctxt.outer() == unresolved_mark
        }
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Minus,
            arg,
            ..
        }) => matches!(&**arg, Expr::Ident(ident)
            if ident.sym == "Infinity" && ident.ctxt.outer() == unresolved_mark),
        _ => false,
    }
}

fn is_flippable_literal_like(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Lit(Lit::Null(_))
        | Expr::Lit(Lit::Bool(_))
        | Expr::Lit(Lit::Num(_))
        | Expr::Lit(Lit::Str(_))
        | Expr::Lit(Lit::BigInt(_)) => true,
        Expr::Tpl(tpl) => tpl.exprs.is_empty(),
        Expr::Ident(ident) => {
            matches!(ident.sym.as_ref(), "undefined" | "NaN" | "Infinity")
                && ident.ctxt.outer() == unresolved_mark
        }
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Void,
            arg,
            ..
        }) => {
            matches!(&**arg, Expr::Lit(Lit::Num(_)))
        }
        // !0 and !1 (pre-UnminifyBooleans forms of true/false)
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg,
            ..
        }) => matches!(&**arg, Expr::Lit(Lit::Num(n)) if n.value == 0.0 || n.value == 1.0),
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Minus,
            arg,
            ..
        }) => match &**arg {
            Expr::Lit(Lit::Num(_)) | Expr::Lit(Lit::BigInt(_)) => true,
            Expr::Ident(ident) => ident.sym == "Infinity" && ident.ctxt.outer() == unresolved_mark,
            _ => false,
        },
        _ => false,
    }
}
