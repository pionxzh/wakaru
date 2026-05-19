use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmt, Class, Decl, Expr, ForHead, ForInStmt,
    ForOfStmt, ForStmt, Function, Ident, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt,
    UpdateExpr, VarDecl, VarDeclKind,
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
        module
            .body
            .iter()
            .for_each(|item| item.visit_with(&mut collector));
        let assigned = collector.assigned;

        // Detect vars declared inside inner blocks that are referenced outside — those
        // must stay as `var` to preserve the hoisting-based access.
        let mut must_stay_var = collect_block_escaping_vars_module(&module.body);
        must_stay_var.extend(collect_duplicate_decl_bindings_module(&module.body));
        must_stay_var.extend(collect_use_before_decl_vars_module(&module.body, &var_ids));

        // Convert all var decls in module (recursively through blocks, stopping at function boundaries)
        let mut converter = VarConverter {
            assigned: &assigned,
            must_stay_var: &must_stay_var,
            in_block_context: true,
        };
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

        let mut must_stay_var = collect_block_escaping_vars_stmts(&body.stmts);
        must_stay_var.extend(collect_duplicate_decl_bindings_stmts(&body.stmts));
        must_stay_var.extend(collect_use_before_decl_vars_stmts(&body.stmts, &var_ids));

        let mut converter = VarConverter {
            assigned: &assigned,
            must_stay_var: &must_stay_var,
            in_block_context: true,
        };
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

fn collect_duplicate_decl_bindings_module(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut collector = ScopeDeclBindingCounter::default();
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.duplicates()
}

fn collect_duplicate_decl_bindings_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut collector = ScopeDeclBindingCounter::default();
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.duplicates()
}

#[derive(Default)]
struct ScopeDeclBindingCounter {
    counts: HashMap<BindingId, usize>,
}

impl ScopeDeclBindingCounter {
    fn record(&mut self, id: BindingId) {
        *self.counts.entry(id).or_insert(0) += 1;
    }

    fn duplicates(self) -> HashSet<BindingId> {
        self.counts
            .into_iter()
            .filter_map(|(id, count)| (count > 1).then_some(id))
            .collect()
    }
}

impl Visit for ScopeDeclBindingCounter {
    fn visit_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Var(var) if var.kind == VarDeclKind::Var => {
                self.visit_var_decl(var);
            }
            Decl::Fn(function) => {
                self.record((function.ident.sym.clone(), function.ident.ctxt));
            }
            Decl::Class(class) => {
                self.record((class.ident.sym.clone(), class.ident.ctxt));
            }
            _ => {}
        }
    }

    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            for d in &var.decls {
                let mut ids = HashSet::new();
                collect_binding_ids_from_pat(&d.name, &mut ids);
                for id in ids {
                    self.record(id);
                }
            }
        }
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
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
    collect_outside_decl_block_refs_module(items, &block_declared)
}

fn collect_block_escaping_vars_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let block_declared = collect_block_declared_var_ids_stmts(stmts);
    if block_declared.is_empty() {
        return HashSet::new();
    }
    collect_outside_decl_block_refs_stmts(stmts, &block_declared)
}

/// Collect vars declared INSIDE inner blocks (depth > 0), keyed by their declaring block id.
fn collect_block_declared_var_ids_module(items: &[ModuleItem]) -> HashMap<BindingId, usize> {
    let mut c = BlockDeclaredVarCollector::default();
    for item in items {
        item.visit_with(&mut c);
    }
    c.ids_by_block
}

fn collect_block_declared_var_ids_stmts(stmts: &[Stmt]) -> HashMap<BindingId, usize> {
    let mut c = BlockDeclaredVarCollector::default();
    for stmt in stmts {
        stmt.visit_with(&mut c);
    }
    c.ids_by_block
}

#[derive(Default)]
struct BlockDeclaredVarCollector {
    ids_by_block: HashMap<BindingId, usize>,
    block_stack: Vec<usize>,
    next_block_id: usize,
}

impl Visit for BlockDeclaredVarCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            let Some(&block_id) = self.block_stack.last() else {
                return;
            };
            for decl in &var.decls {
                let mut ids = HashSet::new();
                collect_binding_ids_from_pat(&decl.name, &mut ids);
                for id in ids {
                    self.ids_by_block.insert(id, block_id);
                }
            }
        }
        // Don't recurse into var decl children
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        let block_id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(block_id);
        block.visit_children_with(self);
        self.block_stack.pop();
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

fn collect_outside_decl_block_refs_module(
    items: &[ModuleItem],
    decl_blocks: &HashMap<BindingId, usize>,
) -> HashSet<BindingId> {
    let mut c = RefOutsideDeclBlockCollector::new(decl_blocks);
    for item in items {
        item.visit_with(&mut c);
    }
    c.refs_outside
}

fn collect_outside_decl_block_refs_stmts(
    stmts: &[Stmt],
    decl_blocks: &HashMap<BindingId, usize>,
) -> HashSet<BindingId> {
    let mut c = RefOutsideDeclBlockCollector::new(decl_blocks);
    for stmt in stmts {
        stmt.visit_with(&mut c);
    }
    c.refs_outside
}

struct RefOutsideDeclBlockCollector<'a> {
    decl_blocks: &'a HashMap<BindingId, usize>,
    refs_outside: HashSet<BindingId>,
    block_stack: Vec<usize>,
    next_block_id: usize,
}

impl<'a> RefOutsideDeclBlockCollector<'a> {
    fn new(decl_blocks: &'a HashMap<BindingId, usize>) -> Self {
        Self {
            decl_blocks,
            refs_outside: HashSet::new(),
            block_stack: Vec::new(),
            next_block_id: 0,
        }
    }
}

impl Visit for RefOutsideDeclBlockCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let binding = (id.sym.clone(), id.ctxt);
        if let Some(&decl_block_id) = self.decl_blocks.get(&binding) {
            if !self.block_stack.contains(&decl_block_id) {
                self.refs_outside.insert(binding);
            }
        }
    }

    fn visit_block_stmt(&mut self, block: &BlockStmt) {
        let block_id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(block_id);
        block.visit_children_with(self);
        self.block_stack.pop();
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

// ============================================================
// Use-before-declaration analysis: find vars that are referenced
// before their `var` declaration in linear statement order.
// These rely on `var` hoisting and must NOT be converted to let/const.
// ============================================================

fn collect_use_before_decl_vars_module(
    items: &[ModuleItem],
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    if var_ids.is_empty() {
        return HashSet::new();
    }

    let mut declared_so_far: HashSet<BindingId> = HashSet::new();
    let mut must_stay: HashSet<BindingId> = HashSet::new();

    for item in items {
        // For direct var declarations: check refs first (catches self-references).
        // For compound statements: pre-add their inner vars so that refs within the
        // same statement see them as declared.
        let is_direct_var = matches!(
            item,
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(v))) if v.kind == VarDeclKind::Var
        );
        if !is_direct_var {
            // First: check for-head init self-references (e.g. `for (var i = i || 0; ...)`)
            // BEFORE adding for-head vars to declared_so_far.
            let self_refs = collect_for_head_self_refs_in_module_item(item, var_ids);
            for r in &self_refs {
                must_stay.insert(r.clone());
            }
            // Then: add all vars (body + for-head) so body refs to for-head vars pass.
            collect_var_decl_ids_in_module_item(item, &mut declared_so_far);
        }
        let refs = collect_refs_in_module_item(item, var_ids);
        for r in &refs {
            if !declared_so_far.contains(r) {
                must_stay.insert(r.clone());
            }
        }
        // For direct var decls, add them after checking refs.
        if is_direct_var {
            collect_var_decl_ids_in_module_item(item, &mut declared_so_far);
        }
    }

    must_stay
}

fn collect_use_before_decl_vars_stmts(
    stmts: &[Stmt],
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    if var_ids.is_empty() {
        return HashSet::new();
    }

    let mut declared_so_far: HashSet<BindingId> = HashSet::new();
    let mut must_stay: HashSet<BindingId> = HashSet::new();

    for stmt in stmts {
        let is_direct_var = matches!(
            stmt,
            Stmt::Decl(Decl::Var(v)) if v.kind == VarDeclKind::Var
        );
        if !is_direct_var {
            let self_refs = collect_for_head_self_refs_in_stmt(stmt, var_ids);
            for r in &self_refs {
                must_stay.insert(r.clone());
            }
            collect_var_decl_ids_in_stmt(stmt, &mut declared_so_far);
        }
        let refs = collect_refs_in_stmt(stmt, var_ids);
        for r in &refs {
            if !declared_so_far.contains(r) {
                must_stay.insert(r.clone());
            }
        }
        if is_direct_var {
            collect_var_decl_ids_in_stmt(stmt, &mut declared_so_far);
        }
    }

    must_stay
}

fn collect_refs_in_module_item(
    item: &ModuleItem,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    match item {
        ModuleItem::Stmt(stmt) => {
            if let Stmt::Decl(Decl::Var(var)) = stmt {
                if var.kind == VarDeclKind::Var {
                    for d in &var.decls {
                        if let Some(init) = &d.init {
                            init.visit_with(&mut collector);
                        }
                        visit_pat_defaults(&d.name, &mut collector);
                    }
                    return collector.refs;
                }
            }
            stmt.visit_with(&mut collector);
        }
        ModuleItem::ModuleDecl(decl) => {
            use swc_core::ecma::ast::{ExportDefaultExpr, ModuleDecl};
            if let ModuleDecl::ExportDefaultExpr(ExportDefaultExpr { expr, .. }) = decl {
                expr.visit_with(&mut collector);
            }
            // `export { x }` is a live binding re-export — it does not
            // evaluate `x` at the statement position, so it is not a
            // use-before-declaration reference.  Skip it.
        }
    }
    collector.refs
}

fn collect_refs_in_stmt(stmt: &Stmt, var_ids: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    if let Stmt::Decl(Decl::Var(var)) = stmt {
        if var.kind == VarDeclKind::Var {
            for d in &var.decls {
                if let Some(init) = &d.init {
                    init.visit_with(&mut collector);
                }
                visit_pat_defaults(&d.name, &mut collector);
            }
            return collector.refs;
        }
    }
    stmt.visit_with(&mut collector);
    collector.refs
}

struct VarRefCollector<'a> {
    var_ids: &'a HashSet<BindingId>,
    refs: HashSet<BindingId>,
}

impl Visit for VarRefCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let binding = (id.sym.clone(), id.ctxt);
        if self.var_ids.contains(&binding) {
            self.refs.insert(binding);
        }
    }

    fn visit_var_decl(&mut self, var: &VarDecl) {
        if var.kind == VarDeclKind::Var {
            for d in &var.decls {
                if let Some(init) = &d.init {
                    init.visit_with(self);
                }
                visit_pat_defaults(&d.name, self);
            }
        } else {
            var.visit_children_with(self);
        }
    }

    // For for-loops: visit init (catches self-references like `var i = i || 0`)
    // and body (catches references to external vars declared after the loop).
    // Skip test/update — they always run after the for-head var is initialized,
    // so refs there are not use-before-decl.
    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        if let Some(init) = &stmt.init {
            init.visit_with(self);
        }
        stmt.body.visit_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

/// Visit default value expressions inside a destructuring pattern.
fn visit_pat_defaults(pat: &Pat, visitor: &mut VarRefCollector<'_>) {
    match pat {
        Pat::Assign(a) => {
            a.right.visit_with(visitor);
            visit_pat_defaults(&a.left, visitor);
        }
        Pat::Object(op) => {
            for prop in &op.props {
                match prop {
                    swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                        if let swc_core::ecma::ast::PropName::Computed(c) = &kv.key {
                            c.visit_with(visitor);
                        }
                        visit_pat_defaults(&kv.value, visitor);
                    }
                    swc_core::ecma::ast::ObjectPatProp::Assign(a) => {
                        if let Some(default) = &a.value {
                            default.visit_with(visitor);
                        }
                    }
                    swc_core::ecma::ast::ObjectPatProp::Rest(r) => {
                        visit_pat_defaults(&r.arg, visitor);
                    }
                }
            }
        }
        Pat::Array(ap) => {
            for elem in ap.elems.iter().flatten() {
                visit_pat_defaults(elem, visitor);
            }
        }
        Pat::Rest(r) => visit_pat_defaults(&r.arg, visitor),
        _ => {}
    }
}

fn collect_var_decl_ids_in_module_item(item: &ModuleItem, out: &mut HashSet<BindingId>) {
    if let ModuleItem::Stmt(stmt) = item {
        collect_var_decl_ids_in_stmt(stmt, out);
    }
}

/// Collect all var declarations within a statement, recursing into nested blocks
/// (but not functions/arrows/classes) since `var` hoists to the enclosing scope.
fn collect_var_decl_ids_in_stmt(stmt: &Stmt, out: &mut HashSet<BindingId>) {
    let mut collector = ScopeVarIdsCollector::default();
    stmt.visit_with(&mut collector);
    out.extend(collector.ids);
}

/// Collect for-head var bindings whose init expression references themselves.
/// E.g. `for (var i = i || 0; ...)` — `i` is used in its own init.
fn collect_for_head_self_refs_in_module_item(
    item: &ModuleItem,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    if let ModuleItem::Stmt(stmt) = item {
        return collect_for_head_self_refs_in_stmt(stmt, var_ids);
    }
    HashSet::new()
}

fn collect_for_head_self_refs_in_stmt(
    stmt: &Stmt,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut collector = ForHeadSelfRefCollector {
        var_ids,
        self_refs: HashSet::new(),
    };
    stmt.visit_with(&mut collector);
    collector.self_refs
}

struct ForHeadSelfRefCollector<'a> {
    var_ids: &'a HashSet<BindingId>,
    self_refs: HashSet<BindingId>,
}

impl ForHeadSelfRefCollector<'_> {
    fn check_var_decl_self_refs(&mut self, var: &VarDecl) {
        if var.kind != VarDeclKind::Var {
            return;
        }
        // Process declarators in order. Each init may only reference
        // bindings from EARLIER declarators (already initialized).
        // References to the current or later declarators' bindings are
        // forward references that would TDZ under let/const.
        let mut declared_so_far: HashSet<BindingId> = HashSet::new();
        let mut all_ids: HashSet<BindingId> = HashSet::new();
        for d in &var.decls {
            collect_binding_ids_from_pat(&d.name, &mut all_ids);
        }
        for d in &var.decls {
            let not_yet: HashSet<BindingId> =
                all_ids.difference(&declared_so_far).cloned().collect();
            let mut ref_collector = VarRefCollector {
                var_ids: &not_yet,
                refs: HashSet::new(),
            };
            if let Some(init) = &d.init {
                init.visit_with(&mut ref_collector);
            }
            visit_pat_defaults(&d.name, &mut ref_collector);
            for r in ref_collector.refs {
                if self.var_ids.contains(&r) {
                    self.self_refs.insert(r);
                }
            }
            collect_binding_ids_from_pat(&d.name, &mut declared_so_far);
        }
    }
}

impl Visit for ForHeadSelfRefCollector<'_> {
    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        if let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(var)) = &stmt.init {
            self.check_var_decl_self_refs(var);
        }
        stmt.body.visit_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        if let ForHead::VarDecl(var) = &stmt.left {
            self.check_var_decl_self_refs(var);
        }
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        if let ForHead::VarDecl(var) = &stmt.left {
            self.check_var_decl_self_refs(var);
        }
        stmt.body.visit_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
}

fn collect_assign_target_ids(target: &AssignTarget, out: &mut HashSet<BindingId>) {
    match target {
        AssignTarget::Simple(simple) => {
            if let SimpleAssignTarget::Ident(bi) = simple {
                out.insert((bi.id.sym.clone(), bi.id.ctxt));
            }
        }
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
    var.kind = if any_assigned {
        VarDeclKind::Let
    } else {
        VarDeclKind::Const
    };
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
