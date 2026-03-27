use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, Class, Decl, Expr, ForHead, ForInStmt, ForOfStmt,
    ForStmt, Function, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt, UpdateExpr, VarDecl,
    VarDeclKind, VarDeclOrExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub struct VarDeclToLetConst;

impl VisitMut for VarDeclToLetConst {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Recurse into nested functions first (bottom-up)
        module.visit_mut_children_with(self);

        // Collect all var names at module top level (recursively through blocks)
        let var_names = collect_all_var_names_in_module_items(&module.body);
        if var_names.is_empty() {
            return;
        }

        // Collect assigned names from ALL statements (including nested functions)
        let mut collector = AssignedNamesCollector::default();
        module.body.iter().for_each(|item| item.visit_with(&mut collector));
        let assigned = collector.assigned;

        // Convert all var decls in module (recursively through blocks, stopping at function boundaries)
        let mut converter = VarConverter { assigned: &assigned, in_block_context: true };
        module.visit_mut_with(&mut converter);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse into children first
        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        // Collect all var names in this function scope (recursively through blocks)
        let var_names = collect_all_var_names_in_stmts(&body.stmts);
        if var_names.is_empty() {
            return;
        }

        let mut collector = AssignedNamesCollector::default();
        body.stmts.iter().for_each(|s| s.visit_with(&mut collector));
        let assigned = collector.assigned;

        let mut converter = VarConverter { assigned: &assigned, in_block_context: true };
        body.visit_mut_with(&mut converter);
    }
}

// ============================================================
// Collect var names declared at this scope level (recursively
// through blocks, but NOT into nested functions)
// ============================================================

fn collect_all_var_names_in_stmts(stmts: &[Stmt]) -> HashSet<Atom> {
    let mut collector = ScopeVarNamesCollector::default();
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.names
}

fn collect_all_var_names_in_module_items(items: &[ModuleItem]) -> HashSet<Atom> {
    let mut collector = ScopeVarNamesCollector::default();
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.names
}

/// Collects all `var` declaration names within the current function scope,
/// recursing into blocks but NOT into nested functions/arrows/classes.
#[derive(Default)]
struct ScopeVarNamesCollector {
    names: HashSet<Atom>,
}

impl Visit for ScopeVarNamesCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            for decl in &var.decls {
                collect_pat_names(&decl.name, &mut self.names);
            }
        }
        // Don't recurse — VarDecl children are patterns/inits, not nested stmts
    }

    // Stop at function boundaries
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
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

    // For-in/for-of loop variables are implicitly reassigned each iteration
    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        if let ForHead::VarDecl(var) = &stmt.left {
            if var.kind == VarDeclKind::Var {
                for decl in &var.decls {
                    collect_pat_names(&decl.name, &mut self.assigned);
                }
            }
        }
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        if let ForHead::VarDecl(var) = &stmt.left {
            if var.kind == VarDeclKind::Var {
                for decl in &var.decls {
                    collect_pat_names(&decl.name, &mut self.assigned);
                }
            }
        }
        stmt.visit_children_with(self);
    }

    // For init vars that are also updated (e.g. for(var i=0; i<n; i++))
    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.visit_children_with(self);
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
// Convert var decls to let/const based on assigned names.
// Uses VisitMut to recurse into all blocks, stopping at
// nested function/arrow/class boundaries.
// Only converts `var` when in a block context where let/const is valid.
// ============================================================

struct VarConverter<'a> {
    assigned: &'a HashSet<Atom>,
    /// true when we're inside a block or at module/function top level —
    /// i.e. `let`/`const` is syntactically valid here.
    in_block_context: bool,
}

impl VisitMut for VarConverter<'_> {
    fn visit_mut_var_decl(&mut self, var: &mut VarDecl) {
        if self.in_block_context {
            convert_single_var_decl(var, self.assigned);
        }
        // Don't recurse — VarDecl children are patterns/inits, not nested stmts
    }

    // A block statement is always a valid let/const context
    fn visit_mut_block_stmt(&mut self, block: &mut swc_core::ecma::ast::BlockStmt) {
        let old = self.in_block_context;
        self.in_block_context = true;
        block.visit_mut_children_with(self);
        self.in_block_context = old;
    }

    // For if/while/do-while/for/labeled: the single-stmt body may NOT be a block.
    // Only allow let/const conversion inside those bodies if they ARE blocks
    // (which visit_mut_block_stmt will handle). We just need to visit children
    // with in_block_context = false so that a bare `var` in the body is skipped.
    fn visit_mut_if_stmt(&mut self, stmt: &mut swc_core::ecma::ast::IfStmt) {
        stmt.test.visit_mut_with(self);
        let old = self.in_block_context;
        // Only a BlockStmt body provides a valid let/const context here
        self.in_block_context = matches!(*stmt.cons, swc_core::ecma::ast::Stmt::Block(_));
        stmt.cons.visit_mut_with(self);
        if let Some(alt) = &mut stmt.alt {
            self.in_block_context = matches!(alt.as_ref(), swc_core::ecma::ast::Stmt::Block(_));
            alt.visit_mut_with(self);
        }
        self.in_block_context = old;
    }

    fn visit_mut_while_stmt(&mut self, stmt: &mut swc_core::ecma::ast::WhileStmt) {
        stmt.test.visit_mut_with(self);
        let old = self.in_block_context;
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_do_while_stmt(&mut self, stmt: &mut swc_core::ecma::ast::DoWhileStmt) {
        let old = self.in_block_context;
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
        stmt.test.visit_mut_with(self);
    }

    fn visit_mut_for_stmt(&mut self, stmt: &mut swc_core::ecma::ast::ForStmt) {
        let old = self.in_block_context;
        // `for (var/let/const x = ...)` init is always a valid let/const context
        self.in_block_context = true;
        if let Some(init) = &mut stmt.init {
            init.visit_mut_with(self);
        }
        if let Some(test) = &mut stmt.test {
            test.visit_mut_with(self);
        }
        if let Some(update) = &mut stmt.update {
            update.visit_mut_with(self);
        }
        // body: only block statement provides valid let/const context
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_for_in_stmt(&mut self, stmt: &mut swc_core::ecma::ast::ForInStmt) {
        let old = self.in_block_context;
        // `for (var/let x in y)` left is always a valid let context
        self.in_block_context = true;
        stmt.left.visit_mut_with(self);
        stmt.right.visit_mut_with(self);
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_for_of_stmt(&mut self, stmt: &mut swc_core::ecma::ast::ForOfStmt) {
        let old = self.in_block_context;
        // `for (var/let x of y)` left is always a valid let context
        self.in_block_context = true;
        stmt.left.visit_mut_with(self);
        stmt.right.visit_mut_with(self);
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_switch_stmt(&mut self, stmt: &mut swc_core::ecma::ast::SwitchStmt) {
        // switch case bodies are valid contexts for let/const
        let old = self.in_block_context;
        self.in_block_context = true;
        stmt.visit_mut_children_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_labeled_stmt(&mut self, stmt: &mut swc_core::ecma::ast::LabeledStmt) {
        // `label: let/const x = ...` is a SyntaxError in ECMAScript,
        // so keep in_block_context false for the direct labeled body.
        // Nested structures (for/while) will reset it themselves.
        let old = self.in_block_context;
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    // Stop at function boundaries (nested functions are handled by VarDeclToLetConst recursion)
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
    fn visit_mut_class(&mut self, _: &mut Class) {}
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

// Keep these for compatibility with any remaining direct callers
#[allow(dead_code)]
fn convert_var_decls_in_stmts(stmts: &mut Vec<Stmt>, assigned: &HashSet<Atom>) {
    for stmt in stmts.iter_mut() {
        convert_var_decl_stmt(stmt, assigned);
    }
}

#[allow(dead_code)]
fn convert_var_decl_stmt(stmt: &mut Stmt, assigned: &HashSet<Atom>) {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => {
            convert_single_var_decl(var, assigned);
        }
        Stmt::For(for_stmt) => {
            if let Some(VarDeclOrExpr::VarDecl(var)) = &mut for_stmt.init {
                convert_single_var_decl(var, assigned);
            }
        }
        Stmt::ForIn(for_in) => {
            if let ForHead::VarDecl(var) = &mut for_in.left {
                convert_single_var_decl(var, assigned);
            }
        }
        Stmt::ForOf(for_of) => {
            if let ForHead::VarDecl(var) = &mut for_of.left {
                convert_single_var_decl(var, assigned);
            }
        }
        _ => {}
    }
}
