use swc_core::ecma::ast::{Expr, ExprStmt, Lit, ModuleItem, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnUseStrict;

impl VisitMut for UnUseStrict {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.visit_mut_children_with(self);
        strip_use_strict_module_directives(items);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);
        strip_use_strict_stmt_directives(stmts);
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

fn strip_use_strict_stmt_directives(stmts: &mut Vec<Stmt>) {
    let mut out = Vec::with_capacity(stmts.len());
    let mut in_directive_prologue = true;

    for stmt in std::mem::take(stmts) {
        if in_directive_prologue {
            if is_string_literal_stmt(&stmt) {
                if is_use_strict_stmt(&stmt) {
                    continue;
                }
            } else {
                in_directive_prologue = false;
            }
        }
        out.push(stmt);
    }

    *stmts = out;
}

fn strip_use_strict_module_directives(items: &mut Vec<ModuleItem>) {
    let mut out = Vec::with_capacity(items.len());
    let mut in_directive_prologue = true;

    for item in std::mem::take(items) {
        if in_directive_prologue {
            match &item {
                ModuleItem::Stmt(stmt) if is_string_literal_stmt(stmt) => {
                    if is_use_strict_stmt(stmt) {
                        continue;
                    }
                }
                ModuleItem::Stmt(_) => {
                    in_directive_prologue = false;
                }
                ModuleItem::ModuleDecl(_) => {
                    in_directive_prologue = false;
                }
            }
        }
        out.push(item);
    }

    *items = out;
}

fn is_string_literal_stmt(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(ExprStmt { expr, .. }) => matches!(&**expr, Expr::Lit(Lit::Str(_))),
        _ => false,
    }
}
