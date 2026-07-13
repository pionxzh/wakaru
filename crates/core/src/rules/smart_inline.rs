use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignExpr, AssignOp, AssignTarget, BindingIdent, BlockStmtOrExpr, Callee,
    CatchClause, ComputedPropName, Constructor, Decl, Expr, ExprStmt, ForInStmt, ForOfStmt,
    Function, GetterProp, Ident, ImportSpecifier, KeyValuePatProp, Lit, MemberExpr, MemberProp,
    Module, ModuleExportName, ModuleItem, Number, ObjectPat, ObjectPatProp, Pat, PropName,
    SetterProp, SimpleAssignTarget, StaticBlock, Stmt, VarDecl, VarDeclKind, VarDeclarator,
    WithStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{is_likely_generated_alias, is_stable_builtin_alias_root};
use crate::utils::paren::{strip_parens, strip_parens_mut};

use super::builtin_aliases::{
    inline_builtin_aliases_stmts, inline_module_builtin_aliases, BuiltinAliasInlineOptions,
};
use super::decl_utils::{
    can_remove_prior_uninitialized_decls, remove_prior_uninitialized_decls, same_ident,
    UninitializedDeclKind,
};
use super::eval_utils::is_direct_eval_call;
use super::helper_matcher::BindingKey;
use super::RewriteLevel;

pub struct SmartInline {
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: HashSet<BindingKey>,
    initialized_binding_scopes: Vec<HashSet<BindingKey>>,
}

impl SmartInline {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark: None,
            use_state_bindings: HashSet::new(),
            initialized_binding_scopes: Vec::new(),
        }
    }

    pub fn new_with_mark(level: RewriteLevel, unresolved_mark: Mark) -> Self {
        Self {
            level,
            unresolved_mark: Some(unresolved_mark),
            use_state_bindings: HashSet::new(),
            initialized_binding_scopes: Vec::new(),
        }
    }
}

impl Default for SmartInline {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for SmartInline {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let previous_use_state_bindings = std::mem::replace(
            &mut self.use_state_bindings,
            collect_use_state_bindings(module),
        );

        // Step 0a: Inline zero-param arrow ident wrappers (const X = () => Y) globally.
        // These are often produced by `require.n` rewriting and used inside nested functions,
        // so they need cross-boundary inlining before per-stmt processing.
        inline_module_arrow_wrappers(module);

        // Step 0b: Inline builtin global aliases (const c = Object.defineProperty) globally.
        // This depends on the standard+ `stable_builtins` assumption: the alias
        // captures the global/property now, while inlining reads it later.
        if self.level >= RewriteLevel::Standard {
            inline_module_builtin_aliases(
                module,
                self.unresolved_mark,
                BuiltinAliasInlineOptions::const_only(),
            );
        }

        let initialized_bindings = self.initialized_bindings();
        process_module_stmt_runs(
            &mut module.body,
            self.level,
            self.unresolved_mark,
            &self.use_state_bindings,
            &initialized_bindings,
        );

        module.visit_mut_children_with(self);
        self.use_state_bindings = previous_use_state_bindings;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let taken = std::mem::take(stmts);
        let initialized_bindings = self.initialized_bindings();
        *stmts = process_stmts(
            taken,
            self.level,
            self.unresolved_mark,
            &self.use_state_bindings,
            &initialized_bindings,
        );
        stmts.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let mut bindings = HashSet::new();
        for param in &function.params {
            collect_pat_write_ids(&param.pat, &mut bindings);
        }
        let mut safety = FunctionEntrySafetyCollector::new(&bindings);
        function.params.visit_with(&mut safety);
        function.body.visit_with(&mut safety);
        let (dynamic_scope, unsafe_bindings) = safety.finish();
        if dynamic_scope {
            bindings.clear();
        } else {
            bindings.retain(|key| !unsafe_bindings.contains(key));
        }
        // Parameters from an enclosing function are outer lexical bindings,
        // not entry bindings of this function's statement lists.
        let outer_scopes = std::mem::replace(&mut self.initialized_binding_scopes, vec![bindings]);
        function.visit_mut_children_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let mut bindings = HashSet::new();
        for param in &arrow.params {
            collect_pat_write_ids(param, &mut bindings);
        }
        let mut safety = FunctionEntrySafetyCollector::new(&bindings);
        arrow.params.visit_with(&mut safety);
        arrow.body.visit_with(&mut safety);
        let (dynamic_scope, unsafe_bindings) = safety.finish();
        if dynamic_scope {
            bindings.clear();
        } else {
            bindings.retain(|key| !unsafe_bindings.contains(key));
        }
        let outer_scopes = std::mem::replace(&mut self.initialized_binding_scopes, vec![bindings]);
        arrow.visit_mut_children_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }

    fn visit_mut_catch_clause(&mut self, catch: &mut CatchClause) {
        let mut bindings = HashSet::new();
        if let Some(param) = &catch.param {
            collect_pat_write_ids(param, &mut bindings);
        }
        let mut safety = FunctionEntrySafetyCollector::new(&bindings);
        catch.body.visit_with(&mut safety);
        let (dynamic_scope, unsafe_bindings) = safety.finish();
        if dynamic_scope {
            bindings.clear();
        } else {
            bindings.retain(|key| !unsafe_bindings.contains(key));
        }
        self.initialized_binding_scopes.push(bindings);
        catch.visit_mut_children_with(self);
        self.initialized_binding_scopes.pop();
    }

    fn visit_mut_constructor(&mut self, constructor: &mut Constructor) {
        // A constructor executes after its surrounding class is created, so
        // outer entry bindings cannot be proven frozen from this statement
        // list alone.
        let outer_scopes = std::mem::take(&mut self.initialized_binding_scopes);
        constructor.visit_mut_children_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }

    fn visit_mut_static_block(&mut self, block: &mut StaticBlock) {
        // Static blocks have their own statement list. Do not inherit a proof
        // whose write ordering was collected in an enclosing list.
        let outer_scopes = std::mem::take(&mut self.initialized_binding_scopes);
        block.visit_mut_children_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }

    fn visit_mut_getter_prop(&mut self, getter: &mut GetterProp) {
        // The key is evaluated in the enclosing activation, but the body runs
        // in a later accessor activation and cannot inherit its entry proofs.
        getter.key.visit_mut_with(self);
        let outer_scopes = std::mem::take(&mut self.initialized_binding_scopes);
        getter.body.visit_mut_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }

    fn visit_mut_setter_prop(&mut self, setter: &mut SetterProp) {
        // Setter parameters and body share the deferred accessor activation;
        // only the computed key belongs to the enclosing activation.
        setter.key.visit_mut_with(self);
        let outer_scopes = std::mem::take(&mut self.initialized_binding_scopes);
        setter.this_param.visit_mut_with(self);
        setter.param.visit_mut_with(self);
        setter.body.visit_mut_with(self);
        self.initialized_binding_scopes = outer_scopes;
    }
}

/// Function parameters are usable as position-independent alias sources only
/// when no arbitrarily-timed nested scope can write them. This fact is
/// function-wide so aliases in nested blocks also see sibling closures.
/// Any AST body that executes at a time other than its lexical position must
/// be visited through `visit_nested`; lexical statement order cannot prove
/// writes inside such a body happen before an alias capture.
struct FunctionEntrySafetyCollector<'a> {
    targets: &'a HashSet<BindingKey>,
    unsafe_bindings: HashSet<BindingKey>,
    nested_depth: usize,
    dynamic_scope: bool,
}

impl<'a> FunctionEntrySafetyCollector<'a> {
    fn new(targets: &'a HashSet<BindingKey>) -> Self {
        Self {
            targets,
            unsafe_bindings: HashSet::new(),
            nested_depth: 0,
            dynamic_scope: false,
        }
    }

    fn finish(self) -> (bool, HashSet<BindingKey>) {
        (self.dynamic_scope, self.unsafe_bindings)
    }

    fn record_nested_write(&mut self, key: BindingKey) {
        if self.nested_depth > 0 && self.targets.contains(&key) {
            self.unsafe_bindings.insert(key);
        }
    }

    fn visit_nested<T>(&mut self, node: &T)
    where
        T: VisitWith<Self>,
    {
        self.nested_depth += 1;
        node.visit_children_with(self);
        self.nested_depth -= 1;
    }
}

impl Visit for FunctionEntrySafetyCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
        self.visit_nested(function);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.visit_nested(arrow);
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        self.visit_nested(class);
    }

    fn visit_getter_prop(&mut self, prop: &GetterProp) {
        // The computed key is evaluated with the object literal; the body is
        // deferred until the property is read.
        prop.key.visit_with(self);
        if let Some(body) = &prop.body {
            self.visit_nested(body);
        }
    }

    fn visit_setter_prop(&mut self, prop: &SetterProp) {
        // The computed key is lexical, while the parameter and body are
        // evaluated only when the property is assigned.
        prop.key.visit_with(self);
        self.nested_depth += 1;
        prop.this_param.visit_with(self);
        prop.param.visit_with(self);
        prop.body.visit_with(self);
        self.nested_depth -= 1;
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let mut targets = HashSet::new();
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                targets.insert((binding.id.sym.clone(), binding.id.ctxt));
            }
            AssignTarget::Pat(pattern) => collect_assign_target_pat_ids(pattern, &mut targets),
            _ => {}
        }
        for key in targets {
            self.record_nested_write(key);
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(id) = update.arg.as_ref() {
            self.record_nested_write((id.sym.clone(), id.ctxt));
        }
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.record_for_head(&stmt.left);
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.record_for_head(&stmt.left);
        stmt.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if is_direct_eval_call(call) {
            self.dynamic_scope = true;
        }
        call.visit_children_with(self);
    }

    fn visit_with_stmt(&mut self, stmt: &WithStmt) {
        self.dynamic_scope = true;
        stmt.visit_children_with(self);
    }
}

impl FunctionEntrySafetyCollector<'_> {
    fn record_for_head(&mut self, head: &swc_core::ecma::ast::ForHead) {
        if let swc_core::ecma::ast::ForHead::Pat(pattern) = head {
            let mut targets = HashSet::new();
            collect_pat_write_ids(pattern, &mut targets);
            for key in targets {
                self.record_nested_write(key);
            }
        }
    }
}

impl SmartInline {
    fn initialized_bindings(&self) -> HashSet<BindingKey> {
        self.initialized_binding_scopes
            .iter()
            .flat_map(|bindings| bindings.iter().cloned())
            .collect()
    }
}

// ============================================================
// Main processing pipeline per statement list
// ============================================================

fn process_module_stmt_runs(
    body: &mut Vec<ModuleItem>,
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: &HashSet<BindingKey>,
    initialized_bindings: &HashSet<BindingKey>,
) {
    let mut new_body = Vec::with_capacity(body.len());
    let mut run = Vec::new();

    for item in std::mem::take(body) {
        match item {
            ModuleItem::Stmt(stmt) => run.push(stmt),
            other => {
                flush_stmt_run(
                    &mut new_body,
                    &mut run,
                    level,
                    unresolved_mark,
                    use_state_bindings,
                    initialized_bindings,
                );
                new_body.push(other);
            }
        }
    }
    flush_stmt_run(
        &mut new_body,
        &mut run,
        level,
        unresolved_mark,
        use_state_bindings,
        initialized_bindings,
    );

    *body = new_body;
}

fn flush_stmt_run(
    new_body: &mut Vec<ModuleItem>,
    run: &mut Vec<Stmt>,
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: &HashSet<BindingKey>,
    initialized_bindings: &HashSet<BindingKey>,
) {
    if run.is_empty() {
        return;
    }

    new_body.extend(
        process_stmts(
            std::mem::take(run),
            level,
            unresolved_mark,
            use_state_bindings,
            initialized_bindings,
        )
        .into_iter()
        .map(ModuleItem::Stmt),
    );
}

fn process_stmts(
    stmts: Vec<Stmt>,
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: &HashSet<BindingKey>,
    initialized_bindings: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    // Pass 0: inline builtin global aliases (const x = Math.floor → replace x with Math.floor)
    // Standard+ only; this assumes globals and builtin properties are not patched
    // between alias capture and use.
    let stmts = if level >= RewriteLevel::Standard {
        inline_builtin_aliases_stmts(
            stmts,
            unresolved_mark,
            BuiltinAliasInlineOptions::const_only(),
        )
    } else {
        stmts
    };
    if level < RewriteLevel::Standard {
        return stmts;
    }
    // Pass 1: inline single-use const declarations (temp vars)
    let stmts = inline_temp_vars(stmts, initialized_bindings, unresolved_mark);
    // Pass 1a: forward adjacent assignment aliases created by async/state-machine
    // recovery: `tmp = expr; target = tmp;` -> `target = expr;`.
    let stmts = forward_adjacent_assignment_aliases(stmts, unresolved_mark);
    // Pass 1b: recover the React useState tuple pattern without making generic
    // numeric property reads iterable.
    let stmts = fold_use_state_tuple_reads(stmts, use_state_bindings);
    let stmts = fold_use_state_assignment_tuple_reads(stmts, use_state_bindings);
    // Pass 2: group consecutive property / array accesses into destructuring

    group_destructuring(stmts, level)
}

// ============================================================
// Module-level arrow wrapper inlining
// Handles: const X = () => Y  (zero-param arrow with ident body)
// These are typically require.n-generated getters used inside nested functions.
// Inlines globally (including across nested function/arrow boundaries).
// After inlining, the second UnIife pass converts (() => Y)(...) → Y(...).
// ============================================================

fn try_extract_zero_param_arrow_ident(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Arrow(arrow) = expr else {
        return None;
    };
    if !arrow.params.is_empty() {
        return None;
    }
    if let BlockStmtOrExpr::Expr(body_expr) = arrow.body.as_ref() {
        if matches!(body_expr.as_ref(), Expr::Ident(_)) {
            return Some(body_expr.clone());
        }
    }
    None
}

#[derive(Default)]
struct GlobalUsageStats {
    callable_uses: usize,
    blocked_uses: usize,
}

fn inline_module_arrow_wrappers(module: &mut Module) {
    // Collect candidates: const X = () => identY at module level (Stmt items only).
    // Use (sym, ctxt) keys so inner-scope variables with the same name are NOT replaced.
    let mut candidates: HashMap<BindingKey, Box<Expr>> = HashMap::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
            continue;
        }
        let decl = &var.decls[0];
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        if try_extract_zero_param_arrow_ident(init).is_some() {
            candidates.insert((bi.id.sym.clone(), bi.id.ctxt), init.clone());
        }
    }

    if candidates.is_empty() {
        return;
    }

    // Count usages globally (including inside nested functions), excluding the definition stmts.
    // Keyed by (sym, ctxt) so only the exact binding is counted.
    let mut usage_count: HashMap<BindingKey, GlobalUsageStats> = candidates
        .keys()
        .map(|k| (k.clone(), GlobalUsageStats::default()))
        .collect();

    for item in &module.body {
        // Skip the definition stmt itself
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if candidates.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        continue;
                    }
                }
            }
        }
        let mut counter = GlobalIdentCounter {
            counts: &mut usage_count,
        };
        item.visit_with(&mut counter);
    }

    // Keep only those with at least one usage elsewhere
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, _)| {
            usage_count
                .get(key)
                .map(|stats| stats.callable_uses >= 1)
                .unwrap_or(false)
        })
        .collect();

    if to_inline.is_empty() {
        return;
    }

    // Remove the definition stmts from the module body
    module.body.retain(|item| {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    let key = (bi.id.sym.clone(), bi.id.ctxt);
                    if to_inline.contains_key(&key)
                        && usage_count
                            .get(&key)
                            .map(|stats| stats.blocked_uses == 0)
                            .unwrap_or(false)
                    {
                        return false;
                    }
                }
            }
        }
        true
    });

    // Replace all usages globally (including inside nested functions)
    let mut inliner = GlobalIdentInliner { map: &to_inline };
    module.visit_mut_with(&mut inliner);
}

/// Counts ident usages everywhere, including inside nested functions/arrows.
/// Only direct call callee positions are safe to inline for wrapper aliases.
struct GlobalIdentCounter<'a> {
    counts: &'a mut HashMap<BindingKey, GlobalUsageStats>,
}

impl Visit for GlobalIdentCounter<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        if let Some(stats) = self.counts.get_mut(&(id.sym.clone(), id.ctxt)) {
            stats.blocked_uses += 1;
        }
    }
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee {
            if let Expr::Ident(id) = callee.as_ref() {
                if let Some(stats) = self.counts.get_mut(&(id.sym.clone(), id.ctxt)) {
                    stats.callable_uses += 1;
                } else {
                    callee.visit_with(self);
                }
            } else {
                callee.visit_with(self);
            }
        }
        call.args.visit_with(self);
    }
    // Skip non-computed member props and prop names (not value positions)
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

/// Replaces direct call callee usages everywhere, including inside nested functions/arrows.
struct GlobalIdentInliner<'a> {
    map: &'a HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for GlobalIdentInliner<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut swc_core::ecma::ast::CallExpr) {
        if let swc_core::ecma::ast::Callee::Expr(callee) = &mut call.callee {
            if let Expr::Ident(id) = callee.as_ref() {
                let key = (id.sym.clone(), id.ctxt);
                if let Some(replacement) = self.map.get(&key) {
                    *callee = replacement.clone();
                }
            }
        }
        call.visit_mut_children_with(self);
    }
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    // NOTE: intentionally does NOT stop at function/arrow/class boundaries
}

// ============================================================
// Pass 1: Temp variable inlining
// ============================================================

fn inline_temp_vars(
    stmts: Vec<Stmt>,
    initialized_bindings: &HashSet<BindingKey>,
    unresolved_mark: Option<Mark>,
) -> Vec<Stmt> {
    // Collect generated-looking `const t = e` aliases. Existing `let`
    // declarations remain: SmartRename can recover meaningful names from
    // their use sites, and predicting that later signal here is not worth a
    // second naming analysis. The generated-name check is also readability
    // policy: meaningful names such as `snapshot` or `store` are recovered
    // signal an unminifier should keep.
    let mut candidates: HashMap<BindingKey, TempCandidate> = HashMap::new();

    for (idx, stmt) in stmts.iter().enumerate() {
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                let decl = &var.decls[0];
                if let Pat::Ident(bi) = &decl.name {
                    if !is_likely_generated_alias(&bi.id.sym) {
                        continue;
                    }
                    // A local binding shadowing a builtin name is handled by
                    // the builtin-alias passes; folding it away here would
                    // reshape the fixtures those passes are keyed on.
                    if is_stable_builtin_alias_root(&bi.id.sym) {
                        continue;
                    }
                    if let Some(init) = &decl.init {
                        if is_simple_expr(init) {
                            let key = (bi.id.sym.clone(), bi.id.ctxt);
                            candidates.insert(
                                key,
                                TempCandidate {
                                    init: init.clone(),
                                    def_idx: idx,
                                    source_is_global_undefined: unresolved_mark.is_some_and(|mark| {
                                        matches!(init.as_ref(), Expr::Ident(id) if id.sym == "undefined" && id.ctxt.outer() == mark)
                                    }),
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    if candidates.is_empty() {
        return stmts;
    }

    let analysis = TempUsageAnalysis::collect(&stmts, &candidates, initialized_bindings);

    // Build set of names to inline (exactly 1 top-level use).
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, candidate)| {
            analysis
                .candidate(key)
                .is_some_and(|usage| usage.can_inline(candidate, &analysis))
        })
        .map(|(key, candidate)| (key, candidate.init))
        .collect();

    if to_inline.is_empty() {
        return stmts;
    }

    // Apply inlining: remove definition stmts, replace single usage with init expr
    let mut result = Vec::new();
    for stmt in stmts {
        // Skip definitions of inlined vars
        if let Stmt::Decl(Decl::Var(var)) = &stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if to_inline.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        continue;
                    }
                }
            }
        }
        let mut stmt = stmt;
        // Replace usages of inlined vars in this statement
        let mut inliner = IdentInliner { map: &to_inline };
        stmt.visit_mut_with(&mut inliner);
        result.push(stmt);
    }

    result
}

fn forward_adjacent_assignment_aliases(
    stmts: Vec<Stmt>,
    unresolved_mark: Option<Mark>,
) -> Vec<Stmt> {
    if stmts.len() < 2 {
        return stmts;
    }

    let mut result = Vec::with_capacity(stmts.len());
    let mut idx = 0;
    while idx < stmts.len() {
        if idx + 1 < stmts.len() {
            if let Some((temp, target)) =
                extract_adjacent_assignment_alias(&stmts[idx], &stmts[idx + 1])
            {
                if can_forward_adjacent_assignment_alias(
                    &stmts,
                    idx,
                    &temp,
                    &target.id,
                    unresolved_mark,
                ) {
                    let mut stmt = stmts[idx].clone();
                    replace_assignment_target_ident(&mut stmt, target);
                    result.push(stmt);
                    idx += 2;
                    continue;
                }
            }
        }

        result.push(stmts[idx].clone());
        idx += 1;
    }

    result
}

fn extract_adjacent_assignment_alias(first: &Stmt, second: &Stmt) -> Option<(Ident, BindingIdent)> {
    let (temp, _) = assignment_stmt_to_ident(first)?;
    let (target, rhs) = assignment_stmt_to_ident(second)?;
    let Expr::Ident(source) = strip_parens(rhs) else {
        return None;
    };
    if !same_ident(&temp.id, source) || same_ident(&temp.id, &target.id) {
        return None;
    }

    Some((temp.id.clone(), target.clone()))
}

fn assignment_stmt_to_ident(stmt: &Stmt) -> Option<(&BindingIdent, &Expr)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };

    Some((target, assign.right.as_ref()))
}

fn replace_assignment_target_ident(stmt: &mut Stmt, target: BindingIdent) {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return;
    };
    let Expr::Assign(assign) = strip_parens_mut(expr) else {
        return;
    };

    assign.left = AssignTarget::Simple(SimpleAssignTarget::Ident(target));
}

fn can_forward_adjacent_assignment_alias(
    stmts: &[Stmt],
    assignment_idx: usize,
    temp: &Ident,
    target: &Ident,
    unresolved_mark: Option<Mark>,
) -> bool {
    if same_ident(temp, target) {
        return false;
    }
    if unresolved_mark.is_some_and(|mark| target.ctxt.outer() == mark) {
        return false;
    }

    let temp_decls = collect_local_var_decl_matches(stmts, temp);
    if temp_decls.len() != 1 {
        return false;
    }
    let temp_decl = temp_decls[0];
    if temp_decl.kind == VarDeclKind::Const
        || temp_decl.has_init
        || temp_decl.stmt_idx > assignment_idx
    {
        return false;
    }

    if !has_local_var_decl(stmts, target, assignment_idx) {
        return false;
    }

    let usage = AssignmentAliasUsage::collect(stmts, temp);
    !usage.has_direct_eval && usage.read_count == 1 && usage.write_count == 1
}

#[derive(Clone, Copy)]
struct LocalVarDeclMatch {
    stmt_idx: usize,
    kind: VarDeclKind,
    has_init: bool,
}

fn collect_local_var_decl_matches(stmts: &[Stmt], ident: &Ident) -> Vec<LocalVarDeclMatch> {
    let mut matches = Vec::new();
    for (stmt_idx, stmt) in stmts.iter().enumerate() {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            if same_ident(&binding.id, ident) {
                matches.push(LocalVarDeclMatch {
                    stmt_idx,
                    kind: var.kind,
                    has_init: decl.init.is_some(),
                });
            }
        }
    }

    matches
}

fn has_local_var_decl(stmts: &[Stmt], ident: &Ident, assignment_idx: usize) -> bool {
    collect_local_var_decl_matches(stmts, ident)
        .into_iter()
        .any(|decl| decl.kind != VarDeclKind::Const && decl.stmt_idx <= assignment_idx)
}

#[derive(Default)]
struct AssignmentAliasUsage {
    read_count: usize,
    write_count: usize,
    has_direct_eval: bool,
}

impl AssignmentAliasUsage {
    fn collect(stmts: &[Stmt], target: &Ident) -> Self {
        let mut usage = Self::default();
        for stmt in stmts {
            stmt.visit_with(&mut AssignmentAliasUsageCollector {
                usage: &mut usage,
                target,
            });
        }
        usage
    }
}

struct AssignmentAliasUsageCollector<'a> {
    usage: &'a mut AssignmentAliasUsage,
    target: &'a Ident,
}

impl AssignmentAliasUsageCollector<'_> {
    fn matches_target(&self, ident: &Ident) -> bool {
        same_ident(ident, self.target)
    }

    fn record_lhs(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                if self.matches_target(&binding.id) {
                    self.usage.write_count += 1;
                }
            }
            other => other.visit_children_with(self),
        }
    }
}

impl Visit for AssignmentAliasUsageCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.matches_target(ident) {
            self.usage.read_count += 1;
        }
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        decl.init.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        self.record_lhs(&assign.left);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = update.arg.as_ref() {
            if self.matches_target(ident) {
                self.usage.read_count += 1;
                self.usage.write_count += 1;
                return;
            }
        }

        update.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if is_direct_eval_call(call) {
            self.usage.has_direct_eval = true;
        }
        call.visit_children_with(self);
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, _: &PropName) {}
}

fn is_simple_expr(expr: &Expr) -> bool {
    // Only inline identifier aliases (const t = someVar), not literals.
    // Literal constants (const g = 'url', const n = 42) are intentionally named
    // and should not be collapsed back into their usage site.
    matches!(expr, Expr::Ident(_))
}

struct TempCandidate {
    init: Box<Expr>,
    def_idx: usize,
    source_is_global_undefined: bool,
}

#[derive(Default)]
struct TempUsageInfo {
    ref_count: usize,
    use_stmt_idx: Option<usize>,
    used_above_decl: bool,
    used_in_nested_function: bool,
    source_mutated_after_def: bool,
    source_written_in_nested_scope: bool,
    dynamic_scope: bool,
    // The candidate binding itself is a write target somewhere (assignment,
    // destructuring assignment, update, for-in/of head). This is invalid for
    // a `const`, but keep the guard so rewriting malformed or partially
    // transformed input cannot move the write onto the source binding.
    mutated: bool,
}

impl TempUsageInfo {
    fn can_inline(&self, candidate: &TempCandidate, analysis: &TempUsageAnalysis) -> bool {
        if self.ref_count != 1
            // Keep long-lived aliases. Even a short generated name can become
            // useful recovered signal when SmartRename sees its later use.
            || self.use_stmt_idx != candidate.def_idx.checked_add(1)
            || self.used_above_decl
            || self.used_in_nested_function
            || self.source_mutated_after_def
            || self.source_written_in_nested_scope
            || self.dynamic_scope
            || self.mutated
        {
            return false;
        }

        if candidate.source_is_global_undefined {
            return true;
        }

        let Expr::Ident(src_id) = candidate.init.as_ref() else {
            return false;
        };
        let src_key = (src_id.sym.clone(), src_id.ctxt);
        let Some(source) = analysis.source_binding(&src_key) else {
            // Imports, unresolved globals, and outer lexical bindings are not
            // frozen by local write analysis.
            return false;
        };

        source.is_safe_before(candidate.def_idx)
    }
}

struct TempUsageAnalysis {
    usage: HashMap<BindingKey, TempUsageInfo>,
    source_bindings: HashMap<BindingKey, SourceBindingInfo>,
}

impl TempUsageAnalysis {
    fn collect(
        stmts: &[Stmt],
        candidates: &HashMap<BindingKey, TempCandidate>,
        initialized_bindings: &HashSet<BindingKey>,
    ) -> Self {
        let mut analysis = Self {
            usage: candidates
                .keys()
                .map(|key| (key.clone(), TempUsageInfo::default()))
                .collect(),
            source_bindings: initialized_bindings
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        SourceBindingInfo {
                            origin: Some(SourceBindingOrigin::Entry),
                            declaration_count: 1,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        };

        let mut source_collector = SourceBindingCollector {
            source_bindings: &mut analysis.source_bindings,
            seen_refs: HashSet::new(),
            stmt_idx: 0,
        };
        for (idx, stmt) in stmts.iter().enumerate() {
            source_collector.stmt_idx = idx;
            stmt.visit_with(&mut source_collector);
        }

        for (idx, stmt) in stmts.iter().enumerate() {
            if stmt_is_temp_definition(stmt, candidates) {
                continue;
            }

            let mut collector = TempUsageCollector {
                analysis: &mut analysis,
                candidates,
                stmt_idx: idx,
            };
            stmt.visit_with(&mut collector);
        }

        analysis
    }

    fn candidate(&self, key: &BindingKey) -> Option<&TempUsageInfo> {
        self.usage.get(key)
    }

    fn source_binding(&self, key: &BindingKey) -> Option<&SourceBindingInfo> {
        self.source_bindings.get(key)
    }
}

#[derive(Default)]
struct SourceBindingInfo {
    decl_idx: Option<usize>,
    origin: Option<SourceBindingOrigin>,
    declaration_count: usize,
    used_above_decl: bool,
}

#[derive(Clone, Copy)]
enum SourceBindingOrigin {
    Entry,
    LocalDecl,
    FunctionDecl,
}

impl SourceBindingInfo {
    fn is_safe_before(&self, candidate_def_idx: usize) -> bool {
        if self.declaration_count != 1 {
            return false;
        }
        match self.origin {
            Some(SourceBindingOrigin::Entry | SourceBindingOrigin::FunctionDecl) => true,
            Some(SourceBindingOrigin::LocalDecl) => {
                !self.used_above_decl
                    && self
                        .decl_idx
                        .is_some_and(|decl_idx| decl_idx <= candidate_def_idx)
            }
            None => false,
        }
    }
}

struct SourceBindingCollector<'a> {
    source_bindings: &'a mut HashMap<BindingKey, SourceBindingInfo>,
    seen_refs: HashSet<BindingKey>,
    stmt_idx: usize,
}

impl Visit for SourceBindingCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        self.seen_refs.insert((id.sym.clone(), id.ctxt));
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        decl.init.visit_with(self);

        let mut bindings = HashSet::new();
        collect_pat_write_ids(&decl.name, &mut bindings);
        for key in bindings {
            let info = self.source_bindings.entry(key.clone()).or_default();
            info.decl_idx.get_or_insert(self.stmt_idx);
            info.origin.get_or_insert(SourceBindingOrigin::LocalDecl);
            info.declaration_count += 1;
            info.used_above_decl |= self.seen_refs.contains(&key);
        }
    }

    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        let key = (decl.ident.sym.clone(), decl.ident.ctxt);
        let info = self.source_bindings.entry(key).or_default();
        info.decl_idx.get_or_insert(0);
        info.origin.get_or_insert(SourceBindingOrigin::FunctionDecl);
        info.declaration_count += 1;
    }

    fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
    fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

fn stmt_is_temp_definition(stmt: &Stmt, candidates: &HashMap<BindingKey, TempCandidate>) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return false;
    }
    let Pat::Ident(bi) = &var.decls[0].name else {
        return false;
    };
    candidates.contains_key(&(bi.id.sym.clone(), bi.id.ctxt))
}

struct TempUsageCollector<'a> {
    analysis: &'a mut TempUsageAnalysis,
    candidates: &'a HashMap<BindingKey, TempCandidate>,
    stmt_idx: usize,
}

impl Visit for TempUsageCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let key = (id.sym.clone(), id.ctxt);
        if let Some(candidate) = self.candidates.get(&key) {
            if let Some(usage) = self.analysis.usage.get_mut(&key) {
                usage.ref_count += 1;
                usage.use_stmt_idx.get_or_insert(self.stmt_idx);
                usage.used_above_decl |= self.stmt_idx < candidate.def_idx;
            }
        }
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};

        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(id)) => {
                self.record_direct_mutation(&(id.sym.clone(), id.ctxt));
                self.record_candidate_mutation(&(id.sym.clone(), id.ctxt));
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(_)) => {}
            AssignTarget::Pat(pat_target) => {
                let mut targets = HashSet::new();
                collect_assign_target_pat_ids(pat_target, &mut targets);
                for key in targets {
                    self.record_direct_mutation(&key);
                    self.record_candidate_mutation(&key);
                }
            }
            _ => {}
        }

        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        match update.arg.as_ref() {
            Expr::Ident(id) => {
                self.record_direct_mutation(&(id.sym.clone(), id.ctxt));
                self.record_candidate_mutation(&(id.sym.clone(), id.ctxt));
            }
            Expr::Member(_) => {}
            _ => {}
        }

        update.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_some() {
            let mut bindings = HashSet::new();
            collect_pat_write_ids(&declarator.name, &mut bindings);
            for key in bindings {
                self.record_direct_mutation(&key);
            }
        }
        declarator.init.visit_with(self);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let mut collector = NestedTempCollector {
            analysis: &mut *self.analysis,
            candidates: self.candidates,
        };
        function.visit_children_with(&mut collector);
    }
    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let mut collector = NestedTempCollector {
            analysis: &mut *self.analysis,
            candidates: self.candidates,
        };
        arrow.visit_children_with(&mut collector);
    }
    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        let mut collector = NestedTempCollector {
            analysis: &mut *self.analysis,
            candidates: self.candidates,
        };
        class.visit_children_with(&mut collector);
    }
    fn visit_getter_prop(&mut self, prop: &GetterProp) {
        prop.key.visit_with(self);
        if let Some(body) = &prop.body {
            let mut collector = NestedTempCollector {
                analysis: &mut *self.analysis,
                candidates: self.candidates,
            };
            body.visit_with(&mut collector);
        }
    }
    fn visit_setter_prop(&mut self, prop: &SetterProp) {
        prop.key.visit_with(self);
        let mut collector = NestedTempCollector {
            analysis: &mut *self.analysis,
            candidates: self.candidates,
        };
        prop.this_param.visit_with(&mut collector);
        prop.param.visit_with(&mut collector);
        prop.body.visit_with(&mut collector);
    }
    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.record_for_head_mutations(&stmt.left);
        stmt.visit_children_with(self);
    }
    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        self.record_for_head_mutations(&stmt.left);
        stmt.visit_children_with(self);
    }
    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if is_direct_eval_call(call) {
            self.record_dynamic_scope();
        }
        call.visit_children_with(self);
    }
    fn visit_with_stmt(&mut self, stmt: &WithStmt) {
        self.record_dynamic_scope();
        stmt.visit_children_with(self);
    }
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

impl TempUsageCollector<'_> {
    fn record_candidate_mutation(&mut self, key: &BindingKey) {
        if let Some(usage) = self.analysis.usage.get_mut(key) {
            usage.mutated = true;
        }
    }

    fn record_for_head_mutations(&mut self, head: &swc_core::ecma::ast::ForHead) {
        if let swc_core::ecma::ast::ForHead::Pat(pat) = head {
            let mut targets = HashSet::new();
            collect_pat_write_ids(pat, &mut targets);
            for key in targets {
                self.record_direct_mutation(&key);
                self.record_candidate_mutation(&key);
            }
        }
    }

    fn record_direct_mutation(&mut self, key: &BindingKey) {
        for (candidate_key, candidate) in self.candidates {
            let Expr::Ident(src_id) = candidate.init.as_ref() else {
                continue;
            };
            if (src_id.sym.clone(), src_id.ctxt) == *key && self.stmt_idx > candidate.def_idx {
                if let Some(usage) = self.analysis.usage.get_mut(candidate_key) {
                    usage.source_mutated_after_def = true;
                }
            }
        }
    }

    fn record_dynamic_scope(&mut self) {
        for usage in self.analysis.usage.values_mut() {
            usage.dynamic_scope = true;
        }
    }
}

fn collect_assign_target_pat_ids(
    pat: &swc_core::ecma::ast::AssignTargetPat,
    out: &mut HashSet<BindingKey>,
) {
    use swc_core::ecma::ast::AssignTargetPat;
    match pat {
        AssignTargetPat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_write_ids(elem, out);
            }
        }
        AssignTargetPat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_write_ids(&kv.value, out),
                    ObjectPatProp::Assign(assign) => {
                        out.insert((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_write_ids(&rest.arg, out),
                }
            }
        }
        AssignTargetPat::Invalid(_) => {}
    }
}

fn collect_pat_write_ids(pat: &Pat, out: &mut HashSet<BindingKey>) {
    match pat {
        Pat::Ident(binding) => {
            out.insert((binding.id.sym.clone(), binding.id.ctxt));
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_write_ids(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_write_ids(&kv.value, out),
                    ObjectPatProp::Assign(assign) => {
                        out.insert((assign.key.sym.clone(), assign.key.ctxt));
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_write_ids(&rest.arg, out),
                }
            }
        }
        Pat::Assign(assign) => collect_pat_write_ids(&assign.left, out),
        Pat::Rest(rest) => collect_pat_write_ids(&rest.arg, out),
        Pat::Expr(expr) => {
            if let Expr::Ident(id) = strip_parens(expr) {
                out.insert((id.sym.clone(), id.ctxt));
            }
        }
        Pat::Invalid(_) => {}
    }
}

struct NestedTempCollector<'a> {
    analysis: &'a mut TempUsageAnalysis,
    candidates: &'a HashMap<BindingKey, TempCandidate>,
}

impl NestedTempCollector<'_> {
    fn record_source_write(&mut self, key: &BindingKey) {
        for (candidate_key, candidate) in self.candidates {
            let Expr::Ident(source) = candidate.init.as_ref() else {
                continue;
            };
            if (source.sym.clone(), source.ctxt) == *key {
                if let Some(usage) = self.analysis.usage.get_mut(candidate_key) {
                    usage.source_written_in_nested_scope = true;
                }
            }
        }
    }

    fn record_dynamic_scope(&mut self) {
        for usage in self.analysis.usage.values_mut() {
            usage.dynamic_scope = true;
        }
    }
}

impl Visit for NestedTempCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let key = (id.sym.clone(), id.ctxt);
        if self.candidates.contains_key(&key) {
            if let Some(usage) = self.analysis.usage.get_mut(&key) {
                usage.used_in_nested_function = true;
            }
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let mut targets = HashSet::new();
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                targets.insert((binding.id.sym.clone(), binding.id.ctxt));
            }
            AssignTarget::Pat(pattern) => collect_assign_target_pat_ids(pattern, &mut targets),
            _ => {}
        }
        for key in targets {
            self.record_source_write(&key);
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(id) = update.arg.as_ref() {
            self.record_source_write(&(id.sym.clone(), id.ctxt));
        }
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        if let swc_core::ecma::ast::ForHead::Pat(pattern) = &stmt.left {
            let mut targets = HashSet::new();
            collect_pat_write_ids(pattern, &mut targets);
            for key in targets {
                self.record_source_write(&key);
            }
        }
        stmt.visit_children_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        if let swc_core::ecma::ast::ForHead::Pat(pattern) = &stmt.left {
            let mut targets = HashSet::new();
            collect_pat_write_ids(pattern, &mut targets);
            for key in targets {
                self.record_source_write(&key);
            }
        }
        stmt.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
        if is_direct_eval_call(call) {
            self.record_dynamic_scope();
        }
        call.visit_children_with(self);
    }

    fn visit_with_stmt(&mut self, stmt: &WithStmt) {
        self.record_dynamic_scope();
        stmt.visit_children_with(self);
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, _: &PropName) {}
}

struct IdentInliner<'a> {
    map: &'a HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for IdentInliner<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Replace ident with its mapped expr before recursing
        if let Expr::Ident(id) = expr {
            if let Some(replacement) = self.map.get(&(id.sym.clone(), id.ctxt)) {
                let original_span = id.span;
                *expr = *replacement.clone();
                set_expr_span(expr, original_span);
                return; // No need to recurse into the replacement
            }
        }
        expr.visit_mut_children_with(self);
    }
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    // Don't inline inside nested functions (would change closure semantics)
    fn visit_mut_function(&mut self, _: &mut swc_core::ecma::ast::Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}
}

// ============================================================
// Pass 2: Group property / array accesses into destructuring
// ============================================================

#[derive(Debug, Clone)]
enum AccessKind {
    /// obj.prop or obj["prop"] — maps to (binding_name, prop_key_string)
    Property {
        binding: Option<BindingIdent>,
        prop_key: PropKey,
        /// Span of the original statement this access was extracted from.
        span: Span,
    },
    /// obj[n] — maps to (binding_name, index)
    Index {
        binding: Option<BindingIdent>,
        index: usize,
        /// Span of the original statement this access was extracted from.
        span: Span,
    },
}

#[derive(Debug, Clone)]
enum PropKey {
    Ident(Atom),
    Str(Atom),
}

fn collect_use_state_bindings(module: &Module) -> HashSet<BindingKey> {
    struct UseStateBindingCollector {
        bindings: HashSet<BindingKey>,
    }

    impl Visit for UseStateBindingCollector {
        fn visit_import_specifier(&mut self, specifier: &ImportSpecifier) {
            if let ImportSpecifier::Named(named) = specifier {
                let imported = named.imported.as_ref().map(import_name_atom);
                if imported.as_ref().unwrap_or(&named.local.sym) == "useState" {
                    self.bindings
                        .insert((named.local.sym.clone(), named.local.ctxt));
                }
            }
        }

        fn visit_object_pat_prop(&mut self, prop: &ObjectPatProp) {
            match prop {
                ObjectPatProp::Assign(assign) => {
                    if assign.key.id.sym == "useState" {
                        self.bindings
                            .insert((assign.key.id.sym.clone(), assign.key.id.ctxt));
                    }
                }
                ObjectPatProp::KeyValue(key_value) => {
                    if prop_name_atom(&key_value.key).as_deref() == Some("useState") {
                        if let Some(binding) = direct_binding_ident_from_pat(&key_value.value) {
                            self.bindings
                                .insert((binding.id.sym.clone(), binding.id.ctxt));
                        }
                    }
                    key_value.value.visit_with(self);
                }
                ObjectPatProp::Rest(rest) => {
                    rest.visit_with(self);
                }
            }
        }
    }

    let mut collector = UseStateBindingCollector {
        bindings: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.bindings
}

fn import_name_atom(name: &ModuleExportName) -> Atom {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.clone(),
        ModuleExportName::Str(value) => value.value.as_str().unwrap_or_default().into(),
    }
}

fn prop_name_atom(prop: &PropName) -> Option<Atom> {
    match prop {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(value) => Some(value.value.as_str()?.into()),
        _ => None,
    }
}

fn direct_binding_ident_from_pat(pat: &Pat) -> Option<&BindingIdent> {
    match pat {
        Pat::Ident(binding) => Some(binding),
        Pat::Assign(assign) => direct_binding_ident_from_pat(&assign.left),
        _ => None,
    }
}

fn fold_use_state_tuple_reads(
    stmts: Vec<Stmt>,
    use_state_bindings: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    let mut stmts = stmts;
    let mut result = Vec::with_capacity(stmts.len());
    let mut i = 0;

    while i < stmts.len() {
        if let Some(stmt) = try_fold_use_state_tuple_at(&stmts, i, use_state_bindings) {
            result.push(stmt);
            i += 3;
        } else {
            result.push(take_stmt(&mut stmts, i));
            i += 1;
        }
    }

    result
}

fn take_stmt(stmts: &mut [Stmt], i: usize) -> Stmt {
    std::mem::replace(
        &mut stmts[i],
        Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: DUMMY_SP }),
    )
}

fn try_fold_use_state_tuple_at(
    stmts: &[Stmt],
    start: usize,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<Stmt> {
    let [decl_stmt, first_read, second_read, rest @ ..] = stmts.get(start..)? else {
        return None;
    };

    let (tuple, init) = try_extract_const_ident_init(decl_stmt)?;
    if !is_use_state_tuple_init_expr(&init, use_state_bindings) {
        return None;
    }

    let Some((first_obj, first_index, Some(first_binding))) = try_extract_index_access(first_read)
    else {
        return None;
    };
    let Some((second_obj, second_index, Some(second_binding))) =
        try_extract_index_access(second_read)
    else {
        return None;
    };

    if first_index != 0 || second_index != 1 {
        return None;
    }
    if !same_ident(&first_obj, &tuple.id) || !same_ident(&second_obj, &tuple.id) {
        return None;
    }
    if ident_is_referenced_in_stmts(&tuple.id, rest) {
        return None;
    }

    let decl_span = decl_stmt.span();
    Some(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: decl_span,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: decl_span,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems: vec![
                    Some(Pat::Ident(first_binding)),
                    Some(Pat::Ident(second_binding)),
                ],
                optional: false,
                type_ann: None,
            }),
            init: Some(init),
            definite: false,
        }],
    }))))
}

struct FoldedUseStateAssignment {
    stmt: Stmt,
    consumed: usize,
    removable_bindings: Vec<Ident>,
    recovered_bindings: Vec<Ident>,
}

fn fold_use_state_assignment_tuple_reads(
    stmts: Vec<Stmt>,
    use_state_bindings: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    let mut result = Vec::with_capacity(stmts.len());
    let mut i = 0;

    while i < stmts.len() {
        if let Some(folded) = try_fold_use_state_assignment_tuple_at(&stmts, i, use_state_bindings)
        {
            let rest = &stmts[i + folded.consumed..];
            if can_remove_prior_uninitialized_decls(
                &result,
                &folded.removable_bindings,
                UninitializedDeclKind::Any,
            ) && !bindings_written_in_stmts(&folded.recovered_bindings, rest)
            {
                let end = result.len();
                remove_prior_uninitialized_decls(
                    &mut result,
                    end,
                    &folded.removable_bindings,
                    UninitializedDeclKind::Any,
                );
                result.push(folded.stmt);
                i += folded.consumed;
                continue;
            }
        }

        result.push(stmts[i].clone());
        i += 1;
    }

    result
}

fn try_fold_use_state_assignment_tuple_at(
    stmts: &[Stmt],
    start: usize,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<FoldedUseStateAssignment> {
    try_fold_use_state_decl_assignment_tuple_at(stmts, start, use_state_bindings)
        .or_else(|| try_fold_use_state_ref_assignment_tuple_at(stmts, start, use_state_bindings))
        .or_else(|| try_fold_use_state_nested_assignment_tuple_at(stmts, start, use_state_bindings))
}

fn try_fold_use_state_decl_assignment_tuple_at(
    stmts: &[Stmt],
    start: usize,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<FoldedUseStateAssignment> {
    let [decl_stmt, first_read, second_read, rest @ ..] = stmts.get(start..)? else {
        return None;
    };
    let (tuple, init) = try_extract_const_ident_init(decl_stmt)?;
    if !is_use_state_tuple_init_expr(&init, use_state_bindings) {
        return None;
    }
    let first = extract_ref_index_assignment(first_read, &tuple.id, 0)?;
    let second = extract_ref_index_assignment(second_read, &tuple.id, 1)?;
    if ident_is_referenced_in_stmts(&tuple.id, rest) {
        return None;
    }

    Some(FoldedUseStateAssignment {
        stmt: build_use_state_assignment_tuple_stmt(
            decl_stmt.span(),
            init,
            first.clone(),
            second.clone(),
        ),
        consumed: 3,
        removable_bindings: vec![first.id.clone(), second.id.clone()],
        recovered_bindings: vec![first.id, second.id],
    })
}

fn try_fold_use_state_ref_assignment_tuple_at(
    stmts: &[Stmt],
    start: usize,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<FoldedUseStateAssignment> {
    let [assign_stmt, first_read, second_read, rest @ ..] = stmts.get(start..)? else {
        return None;
    };
    let (tuple, init) = extract_ident_assignment(assign_stmt)?;
    if !is_use_state_tuple_init_expr(&init, use_state_bindings) {
        return None;
    }
    let first = extract_ref_index_assignment(first_read, &tuple, 0)?;
    let second = extract_ref_index_assignment(second_read, &tuple, 1)?;
    if ident_is_referenced_in_stmts(&tuple, rest) {
        return None;
    }

    Some(FoldedUseStateAssignment {
        stmt: build_use_state_assignment_tuple_stmt(
            assign_stmt.span(),
            init,
            first.clone(),
            second.clone(),
        ),
        consumed: 3,
        removable_bindings: vec![tuple, first.id.clone(), second.id.clone()],
        recovered_bindings: vec![first.id, second.id],
    })
}

fn try_fold_use_state_nested_assignment_tuple_at(
    stmts: &[Stmt],
    start: usize,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<FoldedUseStateAssignment> {
    let [first_stmt, second_read, rest @ ..] = stmts.get(start..)? else {
        return None;
    };
    let (first, tuple, init) = extract_nested_ref_index_assignment(first_stmt, use_state_bindings)?;
    let second = extract_ref_index_assignment(second_read, &tuple, 1)?;
    if ident_is_referenced_in_stmts(&tuple, rest) {
        return None;
    }

    Some(FoldedUseStateAssignment {
        stmt: build_use_state_assignment_tuple_stmt(
            first_stmt.span(),
            init,
            first.clone(),
            second.clone(),
        ),
        consumed: 2,
        removable_bindings: vec![tuple, first.id.clone(), second.id.clone()],
        recovered_bindings: vec![first.id, second.id],
    })
}

fn build_use_state_assignment_tuple_stmt(
    span: Span,
    init: Box<Expr>,
    first: BindingIdent,
    second: BindingIdent,
) -> Stmt {
    Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems: vec![Some(Pat::Ident(first)), Some(Pat::Ident(second))],
                optional: false,
                type_ann: None,
            }),
            init: Some(init),
            definite: false,
        }],
    })))
}

fn extract_ident_assignment(stmt: &Stmt) -> Option<(Ident, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let target = simple_assign_target_ident(assign)?;
    Some((target, assign.right.clone()))
}

fn extract_ref_index_assignment(
    stmt: &Stmt,
    ref_ident: &Ident,
    expected_index: usize,
) -> Option<BindingIdent> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let target = simple_assign_target_ident(assign)?;
    let (obj, index) = extract_index_member(strip_parens(assign.right.as_ref()))?;
    if !same_ident(&obj, ref_ident) || index != expected_index {
        return None;
    }
    Some(BindingIdent {
        id: target,
        type_ann: None,
    })
}

fn extract_nested_ref_index_assignment(
    stmt: &Stmt,
    use_state_bindings: &HashSet<BindingKey>,
) -> Option<(BindingIdent, Ident, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr.as_ref()) else {
        return None;
    };
    let first = simple_assign_target_ident(assign)?;
    let Expr::Member(member) = strip_parens(assign.right.as_ref()) else {
        return None;
    };
    let index = member_prop_index(&member.prop)?;
    if index != 0 {
        return None;
    }
    let Expr::Assign(tuple_assign) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    let tuple = simple_assign_target_ident(tuple_assign)?;
    if !is_use_state_tuple_init_expr(tuple_assign.right.as_ref(), use_state_bindings) {
        return None;
    }

    Some((
        BindingIdent {
            id: first,
            type_ann: None,
        },
        tuple,
        tuple_assign.right.clone(),
    ))
}

fn simple_assign_target_ident(assign: &AssignExpr) -> Option<Ident> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &assign.left else {
        return None;
    };
    Some(ident.id.clone())
}

fn extract_index_member(expr: &Expr) -> Option<(Ident, usize)> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return None;
    };
    Some((obj.clone(), member_prop_index(&member.prop)?))
}

fn member_prop_index(prop: &MemberProp) -> Option<usize> {
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(Number { value, .. })) = computed.expr.as_ref() else {
        return None;
    };
    if *value < 0.0 || value.fract() != 0.0 || *value > 10.0 {
        return None;
    }
    Some(*value as usize)
}

fn bindings_written_in_stmts(bindings: &[Ident], stmts: &[Stmt]) -> bool {
    struct WriteFinder<'a> {
        bindings: &'a [Ident],
        found: bool,
    }

    impl WriteFinder<'_> {
        fn matches(&self, ident: &Ident) -> bool {
            self.bindings
                .iter()
                .any(|binding| same_ident(binding, ident))
        }
    }

    impl Visit for WriteFinder<'_> {
        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = &assign.left {
                if self.matches(&ident.id) {
                    self.found = true;
                    return;
                }
            }
            assign.visit_children_with(self);
        }

        fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
            if let Expr::Ident(ident) = update.arg.as_ref() {
                if self.matches(ident) {
                    self.found = true;
                }
            }
        }
    }

    let mut finder = WriteFinder {
        bindings,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn try_extract_const_ident_init(stmt: &Stmt) -> Option<(BindingIdent, Box<Expr>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    Some((binding.clone(), decl.init.clone()?))
}

fn is_use_state_tuple_init_expr(expr: &Expr, use_state_bindings: &HashSet<BindingKey>) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };

    if is_direct_use_state_call(call, use_state_bindings) {
        return true;
    }

    if call.args.len() != 2 {
        return false;
    }
    let Expr::Lit(Lit::Num(length)) = call.args[1].expr.as_ref() else {
        return false;
    };
    if length.value != 2.0 {
        return false;
    }

    is_direct_use_state_call_expr(call.args[0].expr.as_ref(), use_state_bindings)
}

fn is_direct_use_state_call_expr(expr: &Expr, use_state_bindings: &HashSet<BindingKey>) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    is_direct_use_state_call(call, use_state_bindings)
}

fn is_direct_use_state_call(
    call: &swc_core::ecma::ast::CallExpr,
    use_state_bindings: &HashSet<BindingKey>,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };

    match callee.as_ref() {
        Expr::Ident(id) => use_state_bindings.contains(&(id.sym.clone(), id.ctxt)),
        Expr::Member(member) => match &member.prop {
            MemberProp::Ident(prop) => prop.sym == "useState",
            MemberProp::Computed(ComputedPropName { expr, .. }) => {
                let Expr::Lit(Lit::Str(value)) = expr.as_ref() else {
                    return false;
                };
                value.value == "useState"
            }
            _ => false,
        },
        _ => false,
    }
}

fn ident_is_referenced_in_stmts(id: &Ident, stmts: &[Stmt]) -> bool {
    struct IdentRefFinder<'a> {
        target: &'a Ident,
        found: bool,
    }

    impl Visit for IdentRefFinder<'_> {
        fn visit_ident(&mut self, id: &Ident) {
            if same_ident(id, self.target) {
                self.found = true;
            }
        }

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(computed) = prop {
                computed.visit_with(self);
            }
        }

        fn visit_prop_name(&mut self, _: &PropName) {}
    }

    let mut finder = IdentRefFinder {
        target: id,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn group_destructuring(mut stmts: Vec<Stmt>, level: RewriteLevel) -> Vec<Stmt> {
    // Scan for groups of consecutive `const t = obj.prop` / `const t = obj[n]`
    // where `obj` is a plain identifier.
    // Group by the obj name, emit destructuring when group is "flushed".

    let mut result: Vec<Stmt> = Vec::new();
    let mut current_obj: Option<(Ident, Vec<AccessKind>)> = None;
    let mut i = 0;
    let stmts_count = stmts.len();

    while i < stmts_count {
        let stmt = &stmts[i];

        let stmt_span = stmt.span();
        let next_access = try_extract_prop_access(stmt)
            .map(|(obj, key, binding)| {
                (
                    obj,
                    AccessKind::Property {
                        binding,
                        prop_key: key,
                        span: stmt_span,
                    },
                )
            })
            .or_else(|| {
                try_extract_index_access(stmt).map(|(obj, index, binding)| {
                    (
                        obj,
                        AccessKind::Index {
                            binding,
                            index,
                            span: stmt_span,
                        },
                    )
                })
            });

        if let Some((obj_name, access)) = next_access {
            // Don't group built-in globals — `Object.defineProperty(...)` is clearer
            // than `defineProperty(...)` and destructuring can break `this` binding.
            if is_stable_builtin_alias_root(&obj_name.sym) {
                if let Some((obj, acc)) = current_obj.take() {
                    flush_group(&mut result, obj, acc, level);
                }
                result.push(take_stmt(&mut stmts, i));
                i += 1;
                continue;
            }

            match &mut current_obj {
                Some((cur_obj, accesses))
                    if cur_obj.sym == obj_name.sym && cur_obj.ctxt == obj_name.ctxt =>
                {
                    accesses.push(access);
                }
                _ => {
                    if let Some((obj, acc)) = current_obj.take() {
                        flush_group(&mut result, obj, acc, level);
                    }
                    current_obj = Some((obj_name, vec![access]));
                }
            }
            i += 1;
            continue;
        }

        // Non-matching statement: flush current group
        if let Some((obj, acc)) = current_obj.take() {
            flush_group(&mut result, obj, acc, level);
        }
        result.push(take_stmt(&mut stmts, i));
        i += 1;
    }

    if let Some((obj, acc)) = current_obj.take() {
        flush_group(&mut result, obj, acc, level);
    }

    result
}

/// Try to extract `const t = obj.prop`
/// Returns `(obj_ident, prop_key, binding_name)`
fn try_extract_prop_access(stmt: &Stmt) -> Option<(Ident, PropKey, Option<BindingIdent>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let (obj_name, prop_key) = extract_obj_prop(init)?;
    Some((obj_name, prop_key, Some(bi.clone())))
}

fn extract_obj_prop(expr: &Expr) -> Option<(Ident, PropKey)> {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    // obj must be a plain identifier
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    let key = match prop {
        MemberProp::Ident(ident_name) => PropKey::Ident(ident_name.sym.clone()),
        MemberProp::Computed(computed) => {
            // Only handle string literal keys
            let Expr::Lit(Lit::Str(s)) = computed.expr.as_ref() else {
                return None;
            };
            let s_str = s.value.as_str()?.to_string();
            PropKey::Str(s_str.as_str().into())
        }
        _ => return None,
    };
    Some((obj_id.clone(), key))
}

/// Try to extract `const t = obj[n]` where n is a numeric literal ≤10
fn try_extract_index_access(stmt: &Stmt) -> Option<(Ident, usize, Option<BindingIdent>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;

    let Expr::Member(MemberExpr { obj, prop, .. }) = init.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = obj.as_ref() else {
        return None;
    };
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    let Expr::Lit(Lit::Num(Number { value, .. })) = computed.expr.as_ref() else {
        return None;
    };
    let idx = *value as usize;
    if idx > 10 || *value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some((obj_id.clone(), idx, Some(bi.clone())))
}

/// Determine if accesses are all Property or all Index type
fn flush_group(result: &mut Vec<Stmt>, obj: Ident, accesses: Vec<AccessKind>, level: RewriteLevel) {
    if accesses.len() < 2 {
        // Not worth destructuring — emit individually
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }
    // Check consistency: all property or all index
    let all_prop = accesses
        .iter()
        .all(|a| matches!(a, AccessKind::Property { .. }));
    let all_idx = accesses
        .iter()
        .all(|a| matches!(a, AccessKind::Index { .. }));

    if all_prop {
        flush_property_group(result, obj, accesses);
    } else if all_idx {
        if level >= RewriteLevel::Aggressive {
            flush_index_group(result, obj, accesses);
        } else {
            for acc in accesses {
                result.push(acc_to_stmt(&obj, acc));
            }
        }
    } else {
        // Mixed — emit individually
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
    }
}

fn flush_property_group(result: &mut Vec<Stmt>, obj: Ident, accesses: Vec<AccessKind>) {
    if accesses.len() < 2 {
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }

    // Use the first access's original span for the synthesized VarDecl.
    let group_span = accesses
        .first()
        .map(|a| match a {
            AccessKind::Property { span, .. } | AccessKind::Index { span, .. } => *span,
        })
        .unwrap_or(DUMMY_SP);

    // Build ObjectPat
    let mut props: Vec<ObjectPatProp> = Vec::new();

    for acc in &accesses {
        let AccessKind::Property {
            binding, prop_key, ..
        } = acc
        else {
            continue;
        };
        let prop_name: PropName = match prop_key {
            PropKey::Ident(sym) => {
                PropName::Ident(swc_core::ecma::ast::IdentName::new(sym.clone(), DUMMY_SP))
            }
            PropKey::Str(sym) => PropName::Str(swc_core::ecma::ast::Str {
                span: DUMMY_SP,
                value: sym.as_str().into(),
                raw: None,
            }),
        };

        let prop_sym = match prop_key {
            PropKey::Ident(s) => s.clone(),
            PropKey::Str(s) => s.clone(),
        };

        match binding {
            None => {
                // Standalone access: `obj.prop;` → include in destructuring without alias
                props.push(ObjectPatProp::Assign(swc_core::ecma::ast::AssignPatProp {
                    span: DUMMY_SP,
                    key: BindingIdent {
                        id: Ident::new(prop_sym, DUMMY_SP, SyntaxContext::empty()),
                        type_ann: None,
                    },
                    value: None,
                }));
            }
            Some(alias) => {
                if alias.id.sym == prop_sym {
                    // Same name: shorthand
                    props.push(ObjectPatProp::Assign(swc_core::ecma::ast::AssignPatProp {
                        span: DUMMY_SP,
                        key: alias.clone(),
                        value: None,
                    }));
                } else {
                    // Different name: { key: alias }
                    props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                        key: prop_name,
                        value: Box::new(Pat::Ident(alias.clone())),
                    }));
                }
            }
        }
    }

    result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: group_span,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: group_span,
            name: Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Ident(obj))),
            definite: false,
        }],
    }))));
}

fn flush_index_group(result: &mut Vec<Stmt>, obj: Ident, accesses: Vec<AccessKind>) {
    if accesses.len() < 2 {
        for acc in accesses {
            result.push(acc_to_stmt(&obj, acc));
        }
        return;
    }

    // Use the first access's original span for the synthesized VarDecl.
    let group_span = accesses
        .first()
        .map(|a| match a {
            AccessKind::Property { span, .. } | AccessKind::Index { span, .. } => *span,
        })
        .unwrap_or(DUMMY_SP);

    // Find max index
    let max_idx = accesses
        .iter()
        .filter_map(|a| {
            if let AccessKind::Index { index, .. } = a {
                Some(*index)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);

    // Build elems array with holes
    let mut elems: Vec<Option<Pat>> = vec![None; max_idx + 1];
    let non_inlined: Vec<Stmt> = Vec::new();

    for acc in &accesses {
        let AccessKind::Index { binding, index, .. } = acc else {
            continue;
        };
        if let Some(alias) = binding {
            elems[*index] = Some(Pat::Ident(alias.clone()));
        }
    }

    result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: group_span,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: group_span,
            name: Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems,
                optional: false,
                type_ann: None,
            }),
            init: Some(Box::new(Expr::Ident(obj))),
            definite: false,
        }],
    }))));

    result.extend(non_inlined);
}

/// Set the top-level span of an `Expr` to `span`.
/// Covers the variants produced by inlining in this module (Ident, Member).
fn set_expr_span(expr: &mut Expr, span: Span) {
    match expr {
        Expr::Ident(id) => id.span = span,
        Expr::Member(m) => m.span = span,
        _ => {}
    }
}

fn acc_to_stmt(obj: &Ident, acc: AccessKind) -> Stmt {
    match acc {
        AccessKind::Property {
            binding,
            prop_key,
            span: acc_span,
        } => {
            let prop = match &prop_key {
                PropKey::Ident(s) => {
                    MemberProp::Ident(swc_core::ecma::ast::IdentName::new(s.clone(), DUMMY_SP))
                }
                PropKey::Str(s) => MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
                        span: DUMMY_SP,
                        value: s.as_str().into(),
                        raw: None,
                    }))),
                }),
            };
            let member_expr = Expr::Member(MemberExpr {
                span: acc_span,
                obj: Box::new(Expr::Ident(obj.clone())),
                prop,
            });
            match binding {
                None => Stmt::Expr(ExprStmt {
                    span: acc_span,
                    expr: Box::new(member_expr),
                }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: acc_span,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: acc_span,
                        name: Pat::Ident(alias),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
        AccessKind::Index {
            binding,
            index,
            span: acc_span,
        } => {
            let member_expr = Expr::Member(MemberExpr {
                span: acc_span,
                obj: Box::new(Expr::Ident(obj.clone())),
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(Expr::Lit(Lit::Num(Number {
                        span: DUMMY_SP,
                        value: index as f64,
                        raw: None,
                    }))),
                }),
            });
            match binding {
                None => Stmt::Expr(ExprStmt {
                    span: acc_span,
                    expr: Box::new(member_expr),
                }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: acc_span,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: acc_span,
                        name: Pat::Ident(alias),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
    }
}
