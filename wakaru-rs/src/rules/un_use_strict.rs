use swc_core::ecma::ast::{Expr, ExprStmt, Lit, ModuleItem, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnUseStrict;

impl VisitMut for UnUseStrict {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        items.retain(|item| !matches!(item, ModuleItem::Stmt(stmt) if is_use_strict_stmt(stmt)));
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        stmts.retain(|stmt| !is_use_strict_stmt(stmt));
    }
}

fn is_use_strict_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => {
            matches!(&**expr, Expr::Lit(Lit::Str(s)) if s.value == "use strict")
        }
        _ => false,
    }
}
