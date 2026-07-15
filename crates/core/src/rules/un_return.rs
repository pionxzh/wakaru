use swc_core::common::Mark;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, Expr, ExprStmt, Function, ReturnStmt, Stmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::expr_utils::is_unresolved_undefined;

pub struct UnReturn {
    unresolved_mark: Mark,
}

impl UnReturn {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnReturn {
    fn visit_mut_function(&mut self, function: &mut Function) {
        function.visit_mut_children_with(self);
        if let Some(body) = &mut function.body {
            simplify_tail_return(
                &mut body.stmts,
                function.is_async && function.is_generator,
                self.unresolved_mark,
            );
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);
        if let BlockStmtOrExpr::BlockStmt(block) = &mut *arrow.body {
            simplify_tail_return(&mut block.stmts, false, self.unresolved_mark);
        }
    }
}

fn simplify_tail_return(stmts: &mut Vec<Stmt>, preserve_value_return: bool, unresolved_mark: Mark) {
    let Some(last_stmt) = stmts.pop() else {
        return;
    };

    let Stmt::Return(ReturnStmt { span, arg }) = last_stmt else {
        stmts.push(last_stmt);
        return;
    };

    match arg {
        None => {}
        // Async-generator `return expression` awaits its value, even when the
        // expression is `undefined` or `void 0`. Falling through does not add
        // that promise-resolution turn, so only a bare return is redundant.
        Some(expr) if preserve_value_return => {
            stmts.push(Stmt::Return(ReturnStmt {
                span,
                arg: Some(expr),
            }));
        }
        Some(expr) if is_unresolved_undefined(&expr, unresolved_mark) => {}
        Some(expr) => {
            if let Expr::Unary(UnaryExpr {
                op: UnaryOp::Void,
                arg,
                ..
            }) = *expr
            {
                stmts.push(Stmt::Expr(ExprStmt { span, expr: arg }));
            } else {
                stmts.push(Stmt::Return(ReturnStmt {
                    span,
                    arg: Some(expr),
                }));
            }
        }
    }
}
