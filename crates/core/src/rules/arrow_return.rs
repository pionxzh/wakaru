use swc_core::ecma::ast::{ArrowExpr, ArrowFunctionBody, ReturnStmt, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Converts `() => { return expr; }` → `() => expr`.
pub struct ArrowReturn;

impl VisitMut for ArrowReturn {
    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);

        let ArrowFunctionBody::FunctionBody(block) = arrow.body.as_ref() else {
            return;
        };

        if block.stmts.len() != 1 {
            return;
        }

        let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else {
            return;
        };

        *arrow.body = ArrowFunctionBody::Expr(arg.clone());
    }
}
