use swc_core::ecma::ast::{ArrowExpr, BlockStmtOrExpr, ReturnStmt, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `() => { return expr; }` → `() => expr`.
pub struct ArrowReturn;

impl VisitMut for ArrowReturn {
    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);

        let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else { return; };

        if block.stmts.len() != 1 {
            return;
        }

        let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else { return; };

        *arrow.body = BlockStmtOrExpr::Expr(arg.clone());
    }
}
