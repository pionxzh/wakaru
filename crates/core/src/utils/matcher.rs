use swc_core::ecma::ast::{CallExpr, Expr, Lit, UnaryExpr, UnaryOp};

pub struct Matcher;

impl Matcher {
    pub fn is_helper(call: &CallExpr, name: &str) -> bool {
        match &call.callee {
            swc_core::ecma::ast::Callee::Expr(expr) => match &**expr {
                Expr::Ident(ident) => ident.sym.as_ref() == name,
                _ => false,
            },
            _ => false,
        }
    }

    pub fn is_void_zero(expr: &UnaryExpr) -> bool {
        if expr.op != UnaryOp::Void {
            return false;
        }

        matches!(
            &*expr.arg,
            Expr::Lit(Lit::Num(num)) if (num.value - 0.0).abs() < f64::EPSILON
        )
    }
}
