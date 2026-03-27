use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, BlockStmtOrExpr, Decl, DoWhileStmt, EmptyStmt, Expr, ForInStmt,
    ForOfStmt, ForStmt, IfStmt, ReturnStmt, Stmt, VarDeclKind, WhileStmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnCurlyBraces;

impl VisitMut for UnCurlyBraces {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        let taken = std::mem::replace(stmt, Stmt::Empty(EmptyStmt { span: DUMMY_SP }));
        *stmt = match taken {
            Stmt::If(IfStmt {
                span,
                test,
                cons,
                alt,
            }) => {
                let new_cons = wrap_body(cons);
                let new_alt = alt.map(|alt_box| {
                    // Do not wrap an else-if chain – only non-If, non-Block alternates
                    if matches!(*alt_box, Stmt::If(_) | Stmt::Block(_)) {
                        alt_box
                    } else {
                        Box::new(wrap_stmt(*alt_box))
                    }
                });
                Stmt::If(IfStmt {
                    span,
                    test,
                    cons: new_cons,
                    alt: new_alt,
                })
            }
            Stmt::For(ForStmt {
                span,
                init,
                test,
                update,
                body,
            }) => Stmt::For(ForStmt {
                span,
                init,
                test,
                update,
                body: wrap_body(body),
            }),
            Stmt::While(WhileStmt { span, test, body }) => Stmt::While(WhileStmt {
                span,
                test,
                body: wrap_body(body),
            }),
            Stmt::DoWhile(DoWhileStmt { span, test, body }) => Stmt::DoWhile(DoWhileStmt {
                span,
                test,
                body: wrap_body(body),
            }),
            Stmt::ForIn(ForInStmt {
                span,
                left,
                right,
                body,
            }) => Stmt::ForIn(ForInStmt {
                span,
                left,
                right,
                body: wrap_body(body),
            }),
            Stmt::ForOf(ForOfStmt {
                span,
                is_await,
                left,
                right,
                body,
            }) => Stmt::ForOf(ForOfStmt {
                span,
                is_await,
                left,
                right,
                body: wrap_body(body),
            }),
            other => other,
        };
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);

        let taken = std::mem::replace(
            &mut *arrow.body,
            BlockStmtOrExpr::Expr(Box::new(Expr::Lit(swc_core::ecma::ast::Lit::Num(
                swc_core::ecma::ast::Number {
                    span: DUMMY_SP,
                    value: 0.0,
                    raw: None,
                },
            )))),
        );

        *arrow.body = match taken {
            BlockStmtOrExpr::Expr(expr) => {
                BlockStmtOrExpr::BlockStmt(BlockStmt {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    stmts: vec![Stmt::Return(ReturnStmt {
                        span: DUMMY_SP,
                        arg: Some(expr),
                    })],
                })
            }
            other => other,
        };
    }
}

/// Wrap a body statement in a block unless it is already a block or a var declaration.
fn wrap_body(body: Box<Stmt>) -> Box<Stmt> {
    if should_skip_wrapping(&body) {
        body
    } else {
        Box::new(wrap_stmt(*body))
    }
}

fn should_skip_wrapping(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Block(_) => true,
        Stmt::Empty(_) => true,
        Stmt::Decl(Decl::Var(v)) if v.kind == VarDeclKind::Var => true,
        _ => false,
    }
}

fn wrap_stmt(stmt: Stmt) -> Stmt {
    Stmt::Block(BlockStmt {
        span: DUMMY_SP,
        ctxt: Default::default(),
        stmts: vec![stmt],
    })
}
