use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{Bool, EmptyStmt, Expr, ForStmt, Lit, Stmt, WhileStmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnWhileLoop;

impl VisitMut for UnWhileLoop {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
        *stmt = match taken {
            Stmt::For(ForStmt {
                span,
                init: None,
                test,
                update: None,
                body,
            }) => {
                let test_expr = test
                    .unwrap_or_else(|| Box::new(Expr::Lit(Lit::Bool(Bool { span, value: true }))));
                Stmt::While(WhileStmt {
                    span,
                    test: test_expr,
                    body,
                })
            }
            other => other,
        };
    }
}
