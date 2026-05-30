use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmt, Class, Decl, DefaultDecl, ExportSpecifier,
    Expr, ForHead, ForInStmt, ForOfStmt, ForStmt, Function, Ident, Lit, MemberProp, Module,
    ModuleDecl, ModuleExportName, ModuleItem, Pat, SimpleAssignTarget, Stmt, UpdateExpr, VarDecl,
    VarDeclKind, WithStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::BindingId;
use super::eval_utils::{js_source_mentions_binding, strip_parens, DirectEvalAnalyzer};
use super::RewriteLevel;

pub struct VarDeclToLetConst {
    level: RewriteLevel,
}

impl VarDeclToLetConst {
    pub fn new() -> Self {
        Self::new_with_level(RewriteLevel::Standard)
    }

    pub fn new_with_level(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for VarDeclToLetConst {
    fn default() -> Self {
        Self::new()
    }
}

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
        must_stay_var.extend(collect_loop_captured_vars_module(&module.body));
        keep_eval_affected_vars(&module.body, &var_ids, &mut must_stay_var, true);
        keep_global_observed_vars(&module.body, &var_ids, &mut must_stay_var);
        if self.level == RewriteLevel::Minimal {
            must_stay_var.extend(collect_exported_var_bindings_module(&module.body, &var_ids));
        }

        // Convert all var decls in module (recursively through blocks, stopping at function boundaries)
        let mut converter = VarConverter {
            assigned: &assigned,
            must_stay_var: &must_stay_var,
            in_block_context: true,
            in_with_stmt: false,
        };
        module.visit_mut_with(&mut converter);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        // Recurse into children first
        func.visit_mut_children_with(self);

        let mut param_ids = HashSet::new();
        for param in &func.params {
            collect_binding_ids_from_pat(&param.pat, &mut param_ids);
        }

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
        must_stay_var.extend(collect_param_duplicate_var_bindings(
            &param_ids,
            &body.stmts,
        ));
        must_stay_var.extend(collect_use_before_decl_vars_stmts(&body.stmts, &var_ids));
        must_stay_var.extend(collect_loop_captured_vars_stmts(&body.stmts));
        keep_eval_affected_vars(&body.stmts, &var_ids, &mut must_stay_var, false);

        let mut converter = VarConverter {
            assigned: &assigned,
            must_stay_var: &must_stay_var,
            in_block_context: true,
            in_with_stmt: false,
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

fn collect_param_duplicate_var_bindings(
    params: &HashSet<BindingId>,
    stmts: &[Stmt],
) -> HashSet<BindingId> {
    let vars = collect_all_var_ids_in_stmts(stmts);
    params.intersection(&vars).cloned().collect()
}

fn collect_exported_var_bindings_module(
    items: &[ModuleItem],
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut exported = HashSet::new();
    for item in items {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(export) => {
                let Decl::Var(var) = &export.decl else {
                    continue;
                };
                if var.kind != VarDeclKind::Var {
                    continue;
                }
                for decl in &var.decls {
                    collect_binding_ids_from_pat(&decl.name, &mut exported);
                }
            }
            ModuleDecl::ExportNamed(named) if named.src.is_none() => {
                for specifier in &named.specifiers {
                    let ExportSpecifier::Named(specifier) = specifier else {
                        continue;
                    };
                    let ModuleExportName::Ident(local) = &specifier.orig else {
                        continue;
                    };
                    let id = (local.sym.clone(), local.ctxt);
                    if var_ids.contains(&id) {
                        exported.insert(id);
                    }
                }
            }
            _ => {}
        }
    }
    exported
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
        collect_for_head_assignment_ids(&stmt.left, &mut self.assigned);
        visit_for_head_assignment_expressions(&stmt.left, self);
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        collect_for_head_assignment_ids(&stmt.left, &mut self.assigned);
        visit_for_head_assignment_expressions(&stmt.left, self);
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
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

    fn visit_switch_stmt(&mut self, stmt: &swc_core::ecma::ast::SwitchStmt) {
        stmt.discriminant.visit_with(self);
        let block_id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(block_id);
        for case in &stmt.cases {
            case.visit_with(self);
        }
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
    nested_scope_depth: usize,
}

impl<'a> RefOutsideDeclBlockCollector<'a> {
    fn new(decl_blocks: &'a HashMap<BindingId, usize>) -> Self {
        Self {
            decl_blocks,
            refs_outside: HashSet::new(),
            block_stack: Vec::new(),
            next_block_id: 0,
            nested_scope_depth: 0,
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
        if self.nested_scope_depth > 0 {
            block.visit_children_with(self);
            return;
        }

        let block_id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(block_id);
        block.visit_children_with(self);
        self.block_stack.pop();
    }

    fn visit_switch_stmt(&mut self, stmt: &swc_core::ecma::ast::SwitchStmt) {
        stmt.discriminant.visit_with(self);
        let block_id = self.next_block_id;
        self.next_block_id += 1;
        self.block_stack.push(block_id);
        for case in &stmt.cases {
            case.visit_with(self);
        }
        self.block_stack.pop();
    }

    fn visit_function(&mut self, func: &Function) {
        self.nested_scope_depth += 1;
        func.visit_children_with(self);
        self.nested_scope_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, expr: &ArrowExpr) {
        self.nested_scope_depth += 1;
        expr.visit_children_with(self);
        self.nested_scope_depth -= 1;
    }

    fn visit_class(&mut self, class: &Class) {
        self.nested_scope_depth += 1;
        class.visit_children_with(self);
        self.nested_scope_depth -= 1;
    }
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
    let function_refs = collect_hoisted_function_refs_module(items, var_ids);
    analyze_module_items_in_order(
        items,
        var_ids,
        &function_refs,
        &mut declared_so_far,
        &mut must_stay,
    );

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
    let function_refs = collect_hoisted_function_refs_stmts(stmts, var_ids);
    analyze_stmts_in_order(
        stmts,
        var_ids,
        &function_refs,
        &mut declared_so_far,
        &mut must_stay,
    );

    must_stay
}

fn analyze_module_items_in_order(
    items: &[ModuleItem],
    var_ids: &HashSet<BindingId>,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
    declared_so_far: &mut HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    for item in items {
        match item {
            ModuleItem::Stmt(stmt) => {
                analyze_stmt_in_order(stmt, var_ids, function_refs, declared_so_far, must_stay);
            }
            ModuleItem::ModuleDecl(decl) => {
                use swc_core::ecma::ast::{ExportDefaultExpr, ModuleDecl};
                if let ModuleDecl::ExportDefaultExpr(ExportDefaultExpr { expr, .. }) = decl {
                    let mut refs = collect_refs_in_expr(expr, var_ids);
                    refs.extend(collect_called_hoisted_function_refs(expr, function_refs));
                    refs.extend(collect_refs_in_function_like_expr(expr, var_ids));
                    refs.extend(collect_refs_in_nested_function_like_expr(expr, var_ids));
                    mark_refs_before_decl(refs, declared_so_far, must_stay);
                }
                if let ModuleDecl::ExportDefaultDecl(default_decl) = decl {
                    mark_refs_before_decl(
                        collect_refs_in_default_decl(&default_decl.decl, var_ids),
                        declared_so_far,
                        must_stay,
                    );
                }
            }
        }
    }
}

fn analyze_stmts_in_order(
    stmts: &[Stmt],
    var_ids: &HashSet<BindingId>,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
    declared_so_far: &mut HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    for stmt in stmts {
        analyze_stmt_in_order(stmt, var_ids, function_refs, declared_so_far, must_stay);
    }
}

fn analyze_stmt_in_order(
    stmt: &Stmt,
    var_ids: &HashSet<BindingId>,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
    declared_so_far: &mut HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    match stmt {
        Stmt::Block(block) => {
            analyze_stmts_in_order(
                &block.stmts,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::Decl(Decl::Var(var)) => {
            if var.kind == VarDeclKind::Var {
                analyze_var_decl_in_order(var, var_ids, function_refs, declared_so_far, must_stay);
            } else {
                let mut refs = collect_refs_in_var_decl(var, var_ids);
                refs.extend(collect_function_like_refs_in_var_decl(var, var_ids));
                refs.extend(collect_nested_function_like_refs_in_var_decl(var, var_ids));
                mark_refs_before_decl(refs, declared_so_far, must_stay);
            }
        }
        Stmt::Decl(Decl::Class(class_decl)) => {
            mark_refs_before_decl(
                collect_refs_in_class(&class_decl.class, var_ids),
                declared_so_far,
                must_stay,
            );
        }
        Stmt::Decl(Decl::Fn(fn_decl)) => {
            mark_refs_before_decl(
                collect_refs_in_function(&fn_decl.function, var_ids),
                declared_so_far,
                must_stay,
            );
        }
        Stmt::Decl(_) => {}
        Stmt::Expr(expr) => {
            mark_expr_refs_before_decl(
                &expr.expr,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::If(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.test,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            analyze_stmt_in_order(
                &stmt.cons,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            if let Some(alt) = &stmt.alt {
                analyze_stmt_in_order(alt, var_ids, function_refs, declared_so_far, must_stay);
            }
        }
        Stmt::While(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.test,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::DoWhile(stmt) => {
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            mark_expr_refs_before_decl(
                &stmt.test,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::For(stmt) => {
            if let Some(init) = &stmt.init {
                match init {
                    swc_core::ecma::ast::VarDeclOrExpr::VarDecl(var) => {
                        analyze_var_decl_in_order(
                            var,
                            var_ids,
                            function_refs,
                            declared_so_far,
                            must_stay,
                        );
                    }
                    swc_core::ecma::ast::VarDeclOrExpr::Expr(expr) => {
                        mark_expr_refs_before_decl(
                            expr,
                            var_ids,
                            function_refs,
                            declared_so_far,
                            must_stay,
                        );
                    }
                }
            }
            if let Some(test) = &stmt.test {
                mark_expr_refs_before_decl(
                    test,
                    var_ids,
                    function_refs,
                    declared_so_far,
                    must_stay,
                );
            }
            if let Some(update) = &stmt.update {
                mark_expr_refs_before_decl(
                    update,
                    var_ids,
                    function_refs,
                    declared_so_far,
                    must_stay,
                );
            }
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::ForIn(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.right,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            if let ForHead::VarDecl(var) = &stmt.left {
                declare_var_decl_bindings(var, declared_so_far);
            } else {
                mark_refs_before_decl(
                    collect_refs_in_for_head(&stmt.left, var_ids),
                    declared_so_far,
                    must_stay,
                );
            }
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::ForOf(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.right,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            if let ForHead::VarDecl(var) = &stmt.left {
                declare_var_decl_bindings(var, declared_so_far);
            } else {
                mark_refs_before_decl(
                    collect_refs_in_for_head(&stmt.left, var_ids),
                    declared_so_far,
                    must_stay,
                );
            }
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::Switch(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.discriminant,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            for case in &stmt.cases {
                if let Some(test) = &case.test {
                    mark_expr_refs_before_decl(
                        test,
                        var_ids,
                        function_refs,
                        declared_so_far,
                        must_stay,
                    );
                }
                analyze_stmts_in_order(
                    &case.cons,
                    var_ids,
                    function_refs,
                    declared_so_far,
                    must_stay,
                );
            }
        }
        Stmt::Return(stmt) => {
            if let Some(arg) = &stmt.arg {
                mark_expr_refs_before_decl(arg, var_ids, function_refs, declared_so_far, must_stay);
            }
        }
        Stmt::Throw(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.arg,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::Try(stmt) => {
            analyze_stmts_in_order(
                &stmt.block.stmts,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            if let Some(handler) = &stmt.handler {
                analyze_stmts_in_order(
                    &handler.body.stmts,
                    var_ids,
                    function_refs,
                    declared_so_far,
                    must_stay,
                );
            }
            if let Some(finalizer) = &stmt.finalizer {
                analyze_stmts_in_order(
                    &finalizer.stmts,
                    var_ids,
                    function_refs,
                    declared_so_far,
                    must_stay,
                );
            }
        }
        Stmt::Labeled(stmt) => {
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        Stmt::With(stmt) => {
            mark_expr_refs_before_decl(
                &stmt.obj,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
            analyze_stmt_in_order(
                &stmt.body,
                var_ids,
                function_refs,
                declared_so_far,
                must_stay,
            );
        }
        _ => {
            mark_refs_before_decl(
                collect_refs_in_stmt(stmt, var_ids),
                declared_so_far,
                must_stay,
            );
        }
    }
}

fn analyze_var_decl_in_order(
    var: &VarDecl,
    var_ids: &HashSet<BindingId>,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
    declared_so_far: &mut HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    for decl in &var.decls {
        if let Some(init) = &decl.init {
            mark_expr_refs_before_decl(init, var_ids, function_refs, declared_so_far, must_stay);
            let mut current_decl_ids = HashSet::new();
            collect_binding_ids_from_pat(&decl.name, &mut current_decl_ids);
            let mut function_like_refs = collect_refs_in_function_like_expr(init, var_ids);
            function_like_refs.retain(|id| !current_decl_ids.contains(id));
            mark_refs_before_decl(function_like_refs, declared_so_far, must_stay);
            mark_refs_before_decl(
                collect_refs_in_nested_function_like_expr(init, var_ids),
                declared_so_far,
                must_stay,
            );
        }
        let mut default_refs = VarRefCollector {
            var_ids,
            refs: HashSet::new(),
        };
        visit_pat_defaults(&decl.name, &mut default_refs);
        mark_refs_before_decl(default_refs.refs, declared_so_far, must_stay);
        collect_binding_ids_from_pat(&decl.name, declared_so_far);
    }
}

fn declare_var_decl_bindings(var: &VarDecl, declared_so_far: &mut HashSet<BindingId>) {
    for decl in &var.decls {
        collect_binding_ids_from_pat(&decl.name, declared_so_far);
    }
}

fn collect_hoisted_function_refs_module(
    items: &[ModuleItem],
    var_ids: &HashSet<BindingId>,
) -> HashMap<BindingId, HashSet<BindingId>> {
    let mut refs = HashMap::new();
    for item in items {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
                refs.insert(
                    (function.ident.sym.clone(), function.ident.ctxt),
                    collect_refs_in_function(&function.function, var_ids),
                );
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Fn(function) = &export.decl {
                    refs.insert(
                        (function.ident.sym.clone(), function.ident.ctxt),
                        collect_refs_in_function(&function.function, var_ids),
                    );
                }
            }
            _ => {}
        }
    }
    refs
}

fn collect_hoisted_function_refs_stmts(
    stmts: &[Stmt],
    var_ids: &HashSet<BindingId>,
) -> HashMap<BindingId, HashSet<BindingId>> {
    let mut refs = HashMap::new();
    for stmt in stmts {
        if let Stmt::Decl(Decl::Fn(function)) = stmt {
            refs.insert(
                (function.ident.sym.clone(), function.ident.ctxt),
                collect_refs_in_function(&function.function, var_ids),
            );
        }
    }
    refs
}

fn collect_called_hoisted_function_refs(
    expr: &Expr,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
) -> HashSet<BindingId> {
    let mut collector = HoistedFunctionCallCollector {
        function_refs,
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

fn mark_refs_before_decl(
    refs: HashSet<BindingId>,
    declared_so_far: &HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    for r in refs {
        if !declared_so_far.contains(&r) {
            must_stay.insert(r);
        }
    }
}

fn mark_expr_refs_before_decl(
    expr: &Expr,
    var_ids: &HashSet<BindingId>,
    function_refs: &HashMap<BindingId, HashSet<BindingId>>,
    declared_so_far: &HashSet<BindingId>,
    must_stay: &mut HashSet<BindingId>,
) {
    let mut refs = collect_refs_in_expr(expr, var_ids);
    refs.extend(collect_called_hoisted_function_refs(expr, function_refs));
    mark_refs_before_decl(refs, declared_so_far, must_stay);
}

fn collect_refs_in_expr(expr: &Expr, var_ids: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

fn collect_refs_in_class(
    class: &swc_core::ecma::ast::Class,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    class.visit_with(&mut collector);
    collector.refs
}

fn collect_refs_in_default_decl(
    decl: &DefaultDecl,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    match decl {
        DefaultDecl::Class(class) => collect_refs_in_class(&class.class, var_ids),
        DefaultDecl::Fn(function) => {
            let mut refs = collect_refs_in_function(&function.function, var_ids);
            refs.extend(collect_nested_function_like_refs_in_function(
                &function.function,
                var_ids,
            ));
            refs
        }
        DefaultDecl::TsInterfaceDecl(_) => HashSet::new(),
    }
}

fn collect_refs_in_function(
    function: &Function,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    if let Some(body) = &function.body {
        body.visit_with(&mut collector);
    }
    collector.refs
}

fn collect_nested_function_like_refs_in_function(
    function: &Function,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut collector = NestedFunctionLikeRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    if let Some(body) = &function.body {
        body.visit_with(&mut collector);
    }
    collector.refs
}

fn collect_refs_in_function_like_expr(
    expr: &Expr,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    match expr {
        Expr::Fn(fn_expr) => collect_refs_in_function(&fn_expr.function, var_ids),
        Expr::Arrow(arrow) => {
            let mut collector = VarRefCollector {
                var_ids,
                refs: HashSet::new(),
            };
            arrow.body.visit_with(&mut collector);
            collector.refs
        }
        Expr::Paren(paren) => collect_refs_in_function_like_expr(&paren.expr, var_ids),
        _ => HashSet::new(),
    }
}

fn collect_refs_in_nested_function_like_expr(
    expr: &Expr,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    if is_direct_function_like_expr(expr) {
        return HashSet::new();
    }

    let mut collector = NestedFunctionLikeRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

fn is_direct_function_like_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Fn(_) | Expr::Arrow(_) => true,
        Expr::Paren(paren) => is_direct_function_like_expr(&paren.expr),
        _ => false,
    }
}

fn collect_refs_in_for_head(head: &ForHead, var_ids: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    head.visit_with(&mut collector);
    collector.refs
}

fn collect_for_head_assignment_ids(head: &ForHead, out: &mut HashSet<BindingId>) {
    if let ForHead::Pat(pat) = head {
        collect_binding_ids_from_pat(pat, out);
    }
}

fn visit_for_head_assignment_expressions(head: &ForHead, visitor: &mut AssignedIdsCollector) {
    match head {
        ForHead::VarDecl(var) => var.visit_with(visitor),
        ForHead::Pat(pat) => pat.visit_with(visitor),
        _ => {}
    }
}

fn collect_refs_in_var_decl(var: &VarDecl, var_ids: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    for decl in &var.decls {
        if let Some(init) = &decl.init {
            init.visit_with(&mut collector);
        }
        visit_pat_defaults(&decl.name, &mut collector);
    }
    collector.refs
}

fn collect_nested_function_like_refs_in_var_decl(
    var: &VarDecl,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut refs = HashSet::new();
    for decl in &var.decls {
        if let Some(init) = &decl.init {
            refs.extend(collect_refs_in_nested_function_like_expr(init, var_ids));
        }
    }
    refs
}

fn collect_function_like_refs_in_var_decl(
    var: &VarDecl,
    var_ids: &HashSet<BindingId>,
) -> HashSet<BindingId> {
    let mut refs = HashSet::new();
    for decl in &var.decls {
        if let Some(init) = &decl.init {
            refs.extend(collect_refs_in_function_like_expr(init, var_ids));
        }
    }
    refs
}

fn collect_refs_in_stmt(stmt: &Stmt, var_ids: &HashSet<BindingId>) -> HashSet<BindingId> {
    let mut collector = VarRefCollector {
        var_ids,
        refs: HashSet::new(),
    };
    if let Stmt::Decl(Decl::Var(var)) = stmt {
        return collect_refs_in_var_decl(var, var_ids);
    }
    stmt.visit_with(&mut collector);
    collector.refs
}

struct NestedFunctionLikeRefCollector<'a> {
    var_ids: &'a HashSet<BindingId>,
    refs: HashSet<BindingId>,
}

impl Visit for NestedFunctionLikeRefCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
        self.refs
            .extend(collect_refs_in_function(function, self.var_ids));
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let mut collector = VarRefCollector {
            var_ids: self.var_ids,
            refs: HashSet::new(),
        };
        arrow.body.visit_with(&mut collector);
        self.refs.extend(collector.refs);
    }

    fn visit_class(&mut self, _: &Class) {}
}

struct VarRefCollector<'a> {
    var_ids: &'a HashSet<BindingId>,
    refs: HashSet<BindingId>,
}

struct HoistedFunctionCallCollector<'a> {
    function_refs: &'a HashMap<BindingId, HashSet<BindingId>>,
    refs: HashSet<BindingId>,
}

impl Visit for HoistedFunctionCallCollector<'_> {
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(id) = strip_parens(callee.as_ref()) {
                let binding = (id.sym.clone(), id.ctxt);
                if let Some(refs) = self.function_refs.get(&binding) {
                    self.refs.extend(refs.iter().cloned());
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_new_expr(&mut self, new: &swc_core::ecma::ast::NewExpr) {
        if let Expr::Ident(id) = strip_parens(new.callee.as_ref()) {
            let binding = (id.sym.clone(), id.ctxt);
            if let Some(refs) = self.function_refs.get(&binding) {
                self.refs.extend(refs.iter().cloned());
            }
        }
        new.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, _: &Class) {}
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

fn collect_loop_captured_vars_module(items: &[ModuleItem]) -> HashSet<BindingId> {
    let mut collector = LoopCapturedVarCollector::default();
    for item in items {
        item.visit_with(&mut collector);
    }
    collector.captured
}

fn collect_loop_captured_vars_stmts(stmts: &[Stmt]) -> HashSet<BindingId> {
    let mut collector = LoopCapturedVarCollector::default();
    for stmt in stmts {
        stmt.visit_with(&mut collector);
    }
    collector.captured
}

#[derive(Default)]
struct LoopCapturedVarCollector {
    loop_vars_stack: Vec<HashSet<BindingId>>,
    captured: HashSet<BindingId>,
}

impl LoopCapturedVarCollector {
    fn visit_loop<N>(&mut self, node: &N)
    where
        N: VisitWith<ScopeVarIdsCollector> + VisitWith<Self>,
    {
        let mut vars = ScopeVarIdsCollector::default();
        node.visit_with(&mut vars);
        self.loop_vars_stack.push(vars.ids);
        node.visit_children_with(self);
        self.loop_vars_stack.pop();
    }

    fn mark_captures<N>(&mut self, node: &N)
    where
        N: VisitWith<AllIdentRefCollector>,
    {
        if self.loop_vars_stack.is_empty() {
            return;
        }

        let loop_vars = self.current_loop_vars();
        if loop_vars.is_empty() {
            return;
        }

        let mut refs = AllIdentRefCollector::default();
        node.visit_with(&mut refs);
        for id in refs.refs {
            if loop_vars.contains(&id) {
                self.captured.insert(id);
            }
        }
    }

    fn current_loop_vars(&self) -> HashSet<BindingId> {
        self.loop_vars_stack
            .iter()
            .flat_map(|ids| ids.iter().cloned())
            .collect()
    }
}

impl Visit for LoopCapturedVarCollector {
    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        self.visit_loop(stmt);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.visit_loop(stmt);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.visit_loop(stmt);
    }

    fn visit_while_stmt(&mut self, stmt: &swc_core::ecma::ast::WhileStmt) {
        self.visit_loop(stmt);
    }

    fn visit_do_while_stmt(&mut self, stmt: &swc_core::ecma::ast::DoWhileStmt) {
        self.visit_loop(stmt);
    }

    fn visit_function(&mut self, func: &Function) {
        self.mark_captures(func);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.mark_captures(arrow);
    }

    fn visit_class(&mut self, class: &Class) {
        self.mark_captures(class);
    }
}

#[derive(Default)]
struct AllIdentRefCollector {
    refs: HashSet<BindingId>,
}

impl Visit for AllIdentRefCollector {
    fn visit_ident(&mut self, id: &Ident) {
        self.refs.insert((id.sym.clone(), id.ctxt));
    }
}

trait VisitDirectEvalWith {
    fn visit_direct_eval_with(&self, visitor: &mut DirectEvalAnalyzer);
}

impl VisitDirectEvalWith for [ModuleItem] {
    fn visit_direct_eval_with(&self, visitor: &mut DirectEvalAnalyzer) {
        for item in self {
            item.visit_with(visitor);
        }
    }
}

impl VisitDirectEvalWith for [Stmt] {
    fn visit_direct_eval_with(&self, visitor: &mut DirectEvalAnalyzer) {
        for stmt in self {
            stmt.visit_with(visitor);
        }
    }
}

fn keep_eval_affected_vars<T>(
    items: &[T],
    var_ids: &HashSet<BindingId>,
    must_stay_var: &mut HashSet<BindingId>,
    include_indirect_eval: bool,
) where
    [T]: VisitDirectEvalWith,
{
    let mut analyzer = DirectEvalAnalyzer::default();
    items.visit_direct_eval_with(&mut analyzer);

    if analyzer.unknown_direct_eval || (include_indirect_eval && analyzer.unknown_indirect_eval) {
        must_stay_var.extend(var_ids.iter().cloned());
        return;
    }

    let sources = analyzer.known_direct_eval_sources.iter().chain(
        include_indirect_eval
            .then_some(&analyzer.known_indirect_eval_sources)
            .into_iter()
            .flatten(),
    );

    for var_id in var_ids {
        if sources
            .clone()
            .any(|source| js_source_mentions_binding(source, &var_id.0))
        {
            must_stay_var.insert(var_id.clone());
        }
    }
}

fn keep_global_observed_vars(
    items: &[ModuleItem],
    var_ids: &HashSet<BindingId>,
    must_stay_var: &mut HashSet<BindingId>,
) {
    let mut observer = GlobalVarObserver {
        var_ids,
        observed: HashSet::new(),
        function_depth: 0,
    };
    for item in items {
        item.visit_with(&mut observer);
    }
    must_stay_var.extend(observer.observed);
}

struct GlobalVarObserver<'a> {
    var_ids: &'a HashSet<BindingId>,
    observed: HashSet<BindingId>,
    function_depth: usize,
}

impl GlobalVarObserver<'_> {
    fn mark_name(&mut self, name: &Atom) {
        self.observed
            .extend(self.var_ids.iter().filter(|(sym, _)| sym == name).cloned());
    }

    fn mark_refs(&mut self, refs: HashSet<BindingId>) {
        self.observed
            .extend(refs.into_iter().filter(|id| self.var_ids.contains(id)));
    }
}

impl Visit for GlobalVarObserver<'_> {
    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if self.is_global_object_expr(member.obj.as_ref()) {
            if let Some(name) = static_member_prop_name(&member.prop) {
                self.mark_name(&name);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_with_stmt(&mut self, stmt: &WithStmt) {
        stmt.obj.visit_with(self);

        let mut refs = AllIdentRefCollector::default();
        stmt.body.visit_with(&mut refs);
        self.mark_refs(refs.refs);

        stmt.body.visit_with(self);
    }

    fn visit_function(&mut self, function: &Function) {
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
    }
}

impl GlobalVarObserver<'_> {
    fn is_global_object_expr(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(id) if matches!(id.sym.as_ref(), "globalThis" | "window" | "self") => true,
            Expr::This(_) if self.function_depth == 0 => true,
            _ => false,
        }
    }
}

fn static_member_prop_name(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.clone()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(Lit::Str(s)) => s.value.as_str().map(Atom::from),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
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
    /// true when visiting a `with` body. A `var` there may resolve through the
    /// object environment record, while `let`/`const` would always be lexical.
    in_with_stmt: bool,
}

impl VisitMut for VarConverter<'_> {
    fn visit_mut_var_decl(&mut self, var: &mut VarDecl) {
        if self.in_block_context && !self.in_with_stmt {
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

    fn visit_mut_with_stmt(&mut self, stmt: &mut WithStmt) {
        stmt.obj.visit_mut_with(self);
        let old_block = self.in_block_context;
        let old_with = self.in_with_stmt;
        self.in_with_stmt = true;
        self.in_block_context = matches!(*stmt.body, swc_core::ecma::ast::Stmt::Block(_));
        stmt.body.visit_mut_with(self);
        self.in_block_context = old_block;
        self.in_with_stmt = old_with;
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
    if var.decls.iter().any(|d| pat_requires_var_keyword(&d.name)) {
        return;
    }
    if var
        .decls
        .iter()
        .any(|d| pat_has_duplicate_bindings(&d.name))
    {
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
    if var.decls.iter().any(|d| pat_requires_var_keyword(&d.name)) {
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

fn pat_requires_var_keyword(pat: &Pat) -> bool {
    match pat {
        Pat::Ident(bi) => matches!(bi.id.sym.as_ref(), "let"),
        Pat::Array(array) => array.elems.iter().flatten().any(pat_requires_var_keyword),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => pat_requires_var_keyword(&kv.value),
            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                matches!(assign.key.sym.as_ref(), "let")
            }
            swc_core::ecma::ast::ObjectPatProp::Rest(rest) => pat_requires_var_keyword(&rest.arg),
        }),
        Pat::Rest(rest) => pat_requires_var_keyword(&rest.arg),
        Pat::Assign(assign) => pat_requires_var_keyword(&assign.left),
        _ => false,
    }
}

fn pat_has_duplicate_bindings(pat: &Pat) -> bool {
    fn visit_pat(pat: &Pat, seen: &mut HashSet<BindingId>) -> bool {
        match pat {
            Pat::Ident(bi) => !seen.insert((bi.id.sym.clone(), bi.id.ctxt)),
            Pat::Array(array) => array
                .elems
                .iter()
                .flatten()
                .any(|elem| visit_pat(elem, seen)),
            Pat::Object(object) => object.props.iter().any(|prop| match prop {
                swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => visit_pat(&kv.value, seen),
                swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                    !seen.insert((assign.key.sym.clone(), assign.key.ctxt))
                }
                swc_core::ecma::ast::ObjectPatProp::Rest(rest) => visit_pat(&rest.arg, seen),
            }),
            Pat::Rest(rest) => visit_pat(&rest.arg, seen),
            Pat::Assign(assign) => visit_pat(&assign.left, seen),
            _ => false,
        }
    }

    visit_pat(pat, &mut HashSet::new())
}
