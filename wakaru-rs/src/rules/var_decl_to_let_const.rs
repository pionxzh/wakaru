use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, Decl, Expr, ForInStmt, ForOfStmt, ForStmt, Function,
    Module, ModuleItem, Pat, SimpleAssignTarget, Stmt, UpdateExpr, VarDecl, VarDeclKind,
    VarDeclOrExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct VarDeclToLetConst;

impl VisitMut for VarDeclToLetConst {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Recurse into nested functions first (bottom-up)
        module.visit_mut_children_with(self);

        // Collect var names at module top level
        let var_names = collect_var_names_in_module_items(&module.body);
        if var_names.is_empty() {
            return;
        }

        // Collect assigned names from ALL statements (including nested functions)
        let mut collector = AssignedNamesCollector::default();
        module.body.iter().for_each(|item| item.visit_with(&mut collector));
        let assigned = collector.assigned;

        // Mutate var decls
        convert_var_decls_in_module_items(&mut module.body, &assigned);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse into children first
        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        let var_names = collect_var_names_in_stmts(&body.stmts);
        if var_names.is_empty() {
            return;
        }

        let mut collector = AssignedNamesCollector::default();
        body.stmts.iter().for_each(|s| s.visit_with(&mut collector));
        let assigned = collector.assigned;

        convert_var_decls_in_stmts(&mut body.stmts, &assigned);
    }
}

// ============================================================
// Collect var names declared at this scope level (not nested)
// ============================================================

fn collect_var_names_in_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for stmt in stmts {
        collect_var_names_stmt(stmt, &mut names);
    }
    names
}

fn collect_var_names_in_module_items(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => collect_var_names_stmt(stmt, &mut names),
            ModuleItem::ModuleDecl(_) => {}
        }
    }
    names
}

fn collect_var_names_stmt(stmt: &Stmt, out: &mut HashSet<Atom>) {
    match stmt {
        Stmt::Decl(Decl::Var(var)) if var.kind == VarDeclKind::Var => {
            for decl in &var.decls {
                collect_pat_names(&decl.name, out);
            }
        }
        // for (var x ...) — var declared in for init also belongs to this scope
        Stmt::For(ForStmt { init: Some(VarDeclOrExpr::VarDecl(var)), .. })
            if var.kind == VarDeclKind::Var =>
        {
            for decl in &var.decls {
                collect_pat_names(&decl.name, out);
            }
        }
        Stmt::ForIn(ForInStmt { left, .. }) | Stmt::ForOf(ForOfStmt { left, .. }) => {
            if let swc_core::ecma::ast::ForHead::VarDecl(var) = left {
                if var.kind == VarDeclKind::Var {
                    for decl in &var.decls {
                        collect_pat_names(&decl.name, out);
                    }
                }
            }
        }
        // Do NOT recurse into nested functions
        _ => {}
    }
}

fn collect_pat_names(pat: &Pat, out: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(bi) => {
            out.insert(bi.id.sym.clone());
        }
        Pat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                collect_pat_names(elem, out);
            }
        }
        Pat::Object(op) => {
            for prop in &op.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_names(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.insert(a.key.sym.clone());
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_pat_names(&r.arg, out);
                    }
                }
            }
        }
        Pat::Rest(r) => collect_pat_names(&r.arg, out),
        Pat::Assign(a) => collect_pat_names(&a.left, out),
        _ => {}
    }
}

// ============================================================
// Collect all names that appear on the LHS of any assignment
// (including inside nested functions — conservative approach)
// ============================================================

#[derive(Default)]
struct AssignedNamesCollector {
    assigned: HashSet<Atom>,
}

impl Visit for AssignedNamesCollector {
    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        // Collect the LHS name(s)
        collect_assign_target_names(&expr.left, &mut self.assigned);
        // Recurse into children (right side, etc.)
        expr.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, expr: &UpdateExpr) {
        // x++, x-- count as assignments
        if let Expr::Ident(id) = expr.arg.as_ref() {
            self.assigned.insert(id.sym.clone());
        }
        expr.visit_children_with(self);
    }
}

fn collect_assign_target_names(target: &AssignTarget, out: &mut HashSet<Atom>) {
    match target {
        AssignTarget::Simple(simple) => match simple {
            SimpleAssignTarget::Ident(bi) => {
                out.insert(bi.id.sym.clone());
            }
            _ => {}
        },
        AssignTarget::Pat(pat_target) => {
            collect_assign_pat_target_names(pat_target, out);
        }
    }
}

fn collect_assign_pat_target_names(
    pat: &swc_core::ecma::ast::AssignTargetPat,
    out: &mut HashSet<Atom>,
) {
    match pat {
        swc_core::ecma::ast::AssignTargetPat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                collect_pat_names(elem, out);
            }
        }
        swc_core::ecma::ast::AssignTargetPat::Object(op) => {
            for prop in &op.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_pat_names(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.insert(a.key.sym.clone());
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_pat_names(&r.arg, out);
                    }
                }
            }
        }
        swc_core::ecma::ast::AssignTargetPat::Invalid(_) => {}
    }
}

// ============================================================
// Convert var decls to let/const based on assigned names
// ============================================================

fn convert_var_decls_in_stmts(stmts: &mut Vec<Stmt>, assigned: &HashSet<Atom>) {
    for stmt in stmts.iter_mut() {
        convert_var_decl_stmt(stmt, assigned);
    }
}

fn convert_var_decls_in_module_items(items: &mut Vec<ModuleItem>, assigned: &HashSet<Atom>) {
    for item in items.iter_mut() {
        if let ModuleItem::Stmt(stmt) = item {
            convert_var_decl_stmt(stmt, assigned);
        }
    }
}

fn convert_var_decl_stmt(stmt: &mut Stmt, assigned: &HashSet<Atom>) {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => {
            convert_single_var_decl(var, assigned);
        }
        Stmt::For(for_stmt) => {
            if let Some(VarDeclOrExpr::VarDecl(var)) = &mut for_stmt.init {
                convert_single_var_decl(var, assigned);
            }
            // Don't recurse into body — nested functions handle themselves
        }
        Stmt::ForIn(for_in) => {
            if let swc_core::ecma::ast::ForHead::VarDecl(var) = &mut for_in.left {
                convert_single_var_decl(var, assigned);
            }
        }
        Stmt::ForOf(for_of) => {
            if let swc_core::ecma::ast::ForHead::VarDecl(var) = &mut for_of.left {
                convert_single_var_decl(var, assigned);
            }
        }
        _ => {}
    }
}

fn convert_single_var_decl(var: &mut VarDecl, assigned: &HashSet<Atom>) {
    if var.kind != VarDeclKind::Var {
        return;
    }

    // Check all declarators in this VarDecl
    // A VarDecl without init must be let (can't be const)
    let all_have_init = var.decls.iter().all(|d| d.init.is_some());

    if !all_have_init {
        var.kind = VarDeclKind::Let;
        return;
    }

    // Check if any bound name is in the assigned set
    let any_assigned = var.decls.iter().any(|d| {
        let mut names = HashSet::new();
        collect_pat_names(&d.name, &mut names);
        names.iter().any(|n| assigned.contains(n))
    });

    if any_assigned {
        var.kind = VarDeclKind::Let;
    } else {
        var.kind = VarDeclKind::Const;
    }
}
