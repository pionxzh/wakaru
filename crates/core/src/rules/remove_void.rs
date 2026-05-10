use swc_core::ecma::ast::{BindingIdent, Expr, Ident, Lit, Module, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct RemoveVoid;

impl RemoveVoid {
    pub fn should_run(module: &Module) -> bool {
        let mut detector = UndefinedBindingDetector { found: false };
        module.visit_with(&mut detector);
        !detector.found
    }
}

impl VisitMut for RemoveVoid {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Expr::Unary(UnaryExpr { op, arg, span }) = expr {
            if *op == UnaryOp::Void && is_numeric_literal(strip_parens(arg)) {
                *expr = Expr::Ident(Ident::new_no_ctxt("undefined".into(), *span));
            }
        }
    }
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

fn is_numeric_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Num(_)))
}

struct UndefinedBindingDetector {
    found: bool,
}

impl Visit for UndefinedBindingDetector {
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        if binding.id.sym == "undefined" {
            self.found = true;
        }
    }
}
