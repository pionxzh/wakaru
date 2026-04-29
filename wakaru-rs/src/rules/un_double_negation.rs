use swc_core::common::util::take::Take;
use swc_core::ecma::ast::{
    CondExpr, DoWhileStmt, Expr, ForStmt, IfStmt, UnaryExpr, UnaryOp, WhileStmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnDoubleNegation;

impl VisitMut for UnDoubleNegation {
    fn visit_mut_if_stmt(&mut self, stmt: &mut IfStmt) {
        stmt.visit_mut_children_with(self);
        strip_double_bang(&mut stmt.test);
    }

    fn visit_mut_while_stmt(&mut self, stmt: &mut WhileStmt) {
        stmt.visit_mut_children_with(self);
        strip_double_bang(&mut stmt.test);
    }

    fn visit_mut_do_while_stmt(&mut self, stmt: &mut DoWhileStmt) {
        stmt.visit_mut_children_with(self);
        strip_double_bang(&mut stmt.test);
    }

    fn visit_mut_for_stmt(&mut self, stmt: &mut ForStmt) {
        stmt.visit_mut_children_with(self);
        if let Some(test) = &mut stmt.test {
            strip_double_bang(test);
        }
    }

    fn visit_mut_cond_expr(&mut self, expr: &mut CondExpr) {
        expr.visit_mut_children_with(self);
        strip_double_bang(&mut expr.test);
    }
}

fn strip_double_bang(expr: &mut Box<Expr>) {
    if let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg: inner,
        ..
    }) = &mut **expr
    {
        if let Expr::Unary(UnaryExpr {
            op: UnaryOp::Bang,
            arg: innermost,
            ..
        }) = &mut **inner
        {
            let taken = innermost.take();
            **expr = *taken;
        }
    }
}
