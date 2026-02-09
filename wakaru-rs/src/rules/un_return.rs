use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, Expr, ExprStmt, Function, Lit, ReturnStmt, Stmt, UnaryExpr,
    UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnReturn;

impl VisitMut for UnReturn {
    fn visit_mut_function(&mut self, function: &mut Function) {
        function.visit_mut_children_with(self);
        if let Some(body) = &mut function.body {
            simplify_tail_return(&mut body.stmts);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.visit_mut_children_with(self);
        if let BlockStmtOrExpr::BlockStmt(block) = &mut *arrow.body {
            simplify_tail_return(&mut block.stmts);
        }
    }
}

fn simplify_tail_return(stmts: &mut Vec<Stmt>) {
    let Some(last_stmt) = stmts.pop() else {
        return;
    };

    let Stmt::Return(ReturnStmt { span, arg }) = last_stmt else {
        stmts.push(last_stmt);
        return;
    };

    match arg {
        None => {}
        Some(expr) if is_undefined_expr(&expr) => {}
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

fn is_undefined_expr(expr: &Expr) -> bool {
    if matches!(expr, Expr::Ident(ident) if ident.sym == "undefined") {
        return true;
    }

    matches!(
        expr,
        Expr::Unary(UnaryExpr {
            op: UnaryOp::Void,
            arg,
            ..
        }) if matches!(&**arg, Expr::Lit(Lit::Num(num)) if num.value == 0.0)
    )
}
