use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmt, Class, Expr, ForHead, ForInStmt, ForOfStmt,
    ForStmt, Function, Ident, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt, UpdateExpr,
    VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

/// A binding identity: sym + SyntaxContext (set by `resolver()`).
/// Two variables with the same name but different SyntaxContexts are different bindings.
/// This allows scope-aware analysis without relying on string names alone.
type BindingId = (Atom, SyntaxContext);

pub struct VarDeclToLetConst;

impl VisitMut for VarDeclToLetConst {
    fn visit_mut_module(&mut self, module: &mut Module) {
        // Recurse into nested functions first (bottom-up)
        module.visit_mut_children_with(self);

        // Collect all var names at module top level (recursively through blocks)
        let var_ids = collect_all_var_ids_in_module_items(&module.body);
        if var_ids.is_empty() {
            return;
        }

        // Collect assigned binding IDs from ALL statements (including nested functions).
        // Because resolver() has already run, each identifier has a unique SyntaxContext,
        // so we can distinguish bindings with the same name in different scopes.
        let mut collector = AssignedIdsCollector::default();
        module.body.iter().for_each(|item| item.visit_with(&mut collector));
        let assigned = collector.assigned;

        // Detect vars declared inside inner blocks that are referenced outside — those
        // must stay as `var` to preserve the hoisting-based access.
        let must_stay_var = collect_block_escaping_vars_module(&module.body);

        // Convert all var decls in module (recursively through blocks, stopping at function boundaries)
        let mut converter = VarConverter { assigned: &assigned, must_stay_var: &must_stay_var, in_block_context: true };
        module.visit_mut_with(&mut converter);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse into children first
        func.visit_mut_children_with(self);

        let body = match func.body.as_mut() {
            Some(b) => b,
            None => return,
        };

        // Collect all var binding IDs in this function scope (recursively through blocks)
        let var_ids = collect_all_var_ids_in_stmts(&body.stmts);
        if var_ids.is_empty() {
            return;
        }

        let mut collector = AssignedIdsCollector::default();
        body.stmts.iter().for_each(|s| s.visit_with(&mut collector));
        let assigned = collector.assigned;

        let must_stay_var = collect_block_escaping_vars_stmts(&body.stmts);

        let mut converter = VarConverter { assigned: &assigned, must_stay_var: &must_stay_var, in_block_context: true };
        body.visit_mut_with(&mut converter);
    }
}

// ============================================================
// Collect var binding IDs declared at this scope level (recursively
// through blocks, but NOT into nested functions)
// ============================================================

fn collect_all_var_ids_in_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut collector = ScopeVarIdsCollector::default();
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.ids
}

fn collect_all_var_ids_in_module_items(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut collector = ScopeVarIdsCollector::default();
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.ids
}

/// Collects all `var` declaration binding IDs within the current function scope,
/// recursing into blocks but NOT into nested functions/arrows/classes.
#[derive(Default)]
struct ScopeVarIdsCollector {
    ids: HashSet<BindingId>,
}

impl Visit for ScopeVarIdsCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            for decl in &var.decls {
                collect_binding_ids_from_pat(&decl.name, &mut self.ids);
            }
        }
        // Don't recurse — VarDecl children are patterns/inits, not nested stmts
    }

    // Stop at function boundaries
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Collect (sym, ctxt) BindingIds from a pattern (binding position).
fn collect_binding_ids_from_pat(pat: &Pat, out: &mut HashSet<BindingId>) {
    match pat {
        Pat::Ident(bi) => {
            out.insert((bi.id.sym.clone(), bi.id.ctxt));
        }
        Pat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                collect_binding_ids_from_pat(elem, out);
            }
        }
        Pat::Object(op) => {
            for prop in &op.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_binding_ids_from_pat(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.insert((a.key.sym.clone(), a.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_binding_ids_from_pat(&r.arg, out);
                    }
                }
            }
        }
        Pat::Rest(r) => collect_binding_ids_from_pat(&r.arg, out),
        Pat::Assign(a) => collect_binding_ids_from_pat(&a.left, out),
        _ => {}
    }
}

// ============================================================
// Collect all binding IDs that appear on the LHS of any assignment.
// Traverses everything (including nested functions) since a nested function
// assigning to a captured outer variable still affects the outer binding.
// Using (sym, ctxt) ensures we only mark the EXACT binding that is assigned,
// not same-named bindings in other scopes.
// ============================================================

#[derive(Default)]
struct AssignedIdsCollector {
    assigned: HashSet<BindingId>,
}

impl Visit for AssignedIdsCollector {
    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        collect_assign_target_ids(&expr.left, &mut self.assigned);
        expr.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, expr: &UpdateExpr) {
        // x++, x-- count as assignments
        if let Expr::Ident(id) = expr.arg.as_ref() {
            self.assigned.insert((id.sym.clone(), id.ctxt));
        }
        expr.visit_children_with(self);
    }

    // Note: for-in/for-of loop variables are NOT treated as "assigned" here.
    // The loop variable gets a fresh binding each iteration (like a function parameter),
    // so `for (var key in obj)` can safely become `for (const key in obj)` when `key`
    // is not mutated inside the body. The body is still visited so any assignments
    // inside are captured normally.
    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        stmt.visit_children_with(self);
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.visit_children_with(self);
    }
}

// ============================================================
// Block-escape analysis: find vars declared inside an inner block
// that are referenced at the outer (top-level) scope. Those vars
// must stay as `var` to preserve JavaScript's hoisting semantics.
// ============================================================

fn collect_block_escaping_vars_module(items: &[ModuleItem]) -> HashSet<BindingId> {
    let block_declared = collect_block_declared_var_ids_module(items);
    if block_declared.is_empty() {
        return HashSet::new();
    }
    let outer_refs = collect_outer_refs_module(items);
    block_declared.intersection(&outer_refs).cloned().collect()
}

fn collect_block_escaping_vars_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let block_declared = collect_block_declared_var_ids_stmts(stmts);
    if block_declared.is_empty() {
        return HashSet::new();
    }
    let outer_refs = collect_outer_refs_stmts(stmts);
    block_declared.intersection(&outer_refs).cloned().collect()
}

/// Collect vars declared INSIDE inner blocks (depth > 0), stopping at function boundaries.
fn collect_block_declared_var_ids_module(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut c = BlockDeclaredVarCollector { ids: HashSet::new(), depth: 0 };
    for item in items { item.visit_with(&mut c); }
    c.ids
}

fn collect_block_declared_var_ids_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut c = BlockDeclaredVarCollector { ids: HashSet::new(), depth: 0 };
    for stmt in stmts { stmt.visit_with(&mut c); }
    c.ids
}

struct BlockDeclaredVarCollector {
    ids: HashSet<BindingId>,
    depth: usize,
}

impl Visit for BlockDeclaredVarCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var && self.depth > 0 {
            for decl in &var.decls {
                collect_binding_ids_from_pat(&decl.name, &mut self.ids);
            }
        }
        // Don't recurse into var decl children
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        self.depth += 1;
        block.visit_children_with(self);
        self.depth -= 1;
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Collect identifier references that appear at the OUTER scope level (depth == 0),
/// i.e. in top-level statements but NOT inside any nested block.
fn collect_outer_refs_module(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut c = OuterRefCollector { refs: HashSet::new(), depth: 0 };
    for item in items { item.visit_with(&mut c); }
    c.refs
}

fn collect_outer_refs_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut c = OuterRefCollector { refs: HashSet::new(), depth: 0 };
    for stmt in stmts { stmt.visit_with(&mut c); }
    c.refs
}

struct OuterRefCollector {
    refs: HashSet<BindingId>,
    depth: usize,
}

impl Visit for OuterRefCollector {
    fn visit_ident(&mut self, id: &Ident) {
        if self.depth == 0 {
            self.refs.insert((id.sym.clone(), id.ctxt));
        }
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        self.depth += 1;
        block.visit_children_with(self);
        self.depth -= 1;
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

fn collect_assign_target_ids(target: &AssignTarget, out: &mut HashSet<BindingId>) {
    match target {
        AssignTarget::Simple(simple) => match simple {
            SimpleAssignTarget::Ident(bi) => {
                out.insert((bi.id.sym.clone(), bi.id.ctxt));
            }
            _ => {}
        },
        AssignTarget::Pat(pat_target) => {
            collect_assign_pat_target_ids(pat_target, out);
        }
    }
}

fn collect_assign_pat_target_ids(
    pat: &swc_core::ecma::ast::AssignTargetPat,
    out: &mut HashSet<BindingId>,
) {
    match pat {
        swc_core::ecma::ast::AssignTargetPat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                collect_binding_ids_from_pat(elem, out);
            }
        }
        swc_core::ecma::ast::AssignTargetPat::Object(op) => {
            for prop in &op.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        collect_binding_ids_from_pat(&kv.value, out);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        out.insert((a.key.sym.clone(), a.key.ctxt));
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        collect_binding_ids_from_pat(&r.arg, out);
                    }
                }
            }
        }
        swc_core::ecma::ast::AssignTargetPat::Invalid(_) => {}
    }
}

// ============================================================
// Convert var decls to let/const based on assigned binding IDs.
// Uses VisitMut to recurse into all blocks, stopping at
// nested function/arrow/class boundaries.
// Only converts `var` when in a block context where let/const is valid.
// ============================================================

struct VarConverter<'a> {
    assigned: &'a HashSet<BindingId>,
    /// Vars that must remain as `var` because they escape their declaring block
    must_stay_var: &'a HashSet<BindingId>,
    /// true when we're inside a block or at module/function top level —
    /// i.e. `let`/`const` is syntactically valid here.
    in_block_context: bool,
}

impl VisitMut for VarConverter<'_> {
    fn visit_mut_var_decl(&mut self, var: &mut VarDecl) {
        if self.in_block_context {
            // Skip conversion if any declarator must remain as `var` due to block escape
            let any_must_stay = var.decls.iter().any(|d| {
                let mut ids = HashSet::new();
                collect_binding_ids_from_pat(&d.name, &mut ids);
                ids.iter().any(|id| self.must_stay_var.contains(id))
            });
            if !any_must_stay {
                convert_single_var_decl(var, self.assigned);
            }
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
        // For for-in heads, the loop variable has an implicit initializer (the
        // iteration value), so the "no init → let" rule does not apply.
        // Use const if not reassigned inside the body, let otherwise.
        if let ForHead::VarDecl(var) = &mut stmt.left {
            if var.kind == VarDeclKind::Var {
                let any_must_stay = var.decls.iter().any(|d| {
                    let mut ids = HashSet::new();
                    collect_binding_ids_from_pat(&d.name, &mut ids);
                    ids.iter().any(|id| self.must_stay_var.contains(id))
                });
                if !any_must_stay {
                    convert_for_iter_var_decl(var, self.assigned);
                }
            }
        }
        stmt.right.visit_mut_with(self);
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old;
    }

    fn visit_mut_for_of_stmt(&mut self, stmt: &mut swc_core::ecma::ast::ForOfStmt) {
        let old = self.in_block_context;
        if let ForHead::VarDecl(var) = &mut stmt.left {
            if var.kind == VarDeclKind::Var {
                let any_must_stay = var.decls.iter().any(|d| {
                    let mut ids = HashSet::new();
                    collect_binding_ids_from_pat(&d.name, &mut ids);
                    ids.iter().any(|id| self.must_stay_var.contains(id))
                });
                if !any_must_stay {
                    convert_for_iter_var_decl(var, self.assigned);
                }
            }
        }
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
        // Nested structures (for/while/switch) will reset it themselves.
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

/// For `for-in` / `for-of` loop variables: the variable is always "initialized"
/// by the iteration, so we skip the `all_have_init` check and go straight to
/// the assigned-set check. Use `const` if the binding is never reassigned,
/// `let` otherwise.
fn convert_for_iter_var_decl(var: &mut VarDecl, assigned: &HashSet<BindingId>) {
    if var.kind != VarDeclKind::Var {
        return;
    }
    let any_assigned = var.decls.iter().any(|d| {
        let mut ids = HashSet::new();
        collect_binding_ids_from_pat(&d.name, &mut ids);
        ids.iter().any(|id| assigned.contains(id))
    });
    var.kind = if any_assigned { VarDeclKind::Let } else { VarDeclKind::Const };
}

fn convert_single_var_decl(var: &mut VarDecl, assigned: &HashSet<BindingId>) {
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

    // Check if any bound binding ID is in the assigned set
    let any_assigned = var.decls.iter().any(|d| {
        let mut ids = HashSet::new();
        collect_binding_ids_from_pat(&d.name, &mut ids);
        ids.iter().any(|id| assigned.contains(id))
    });

    if any_assigned {
        var.kind = VarDeclKind::Let;
    } else {
        var.kind = VarDeclKind::Const;
    }
}
