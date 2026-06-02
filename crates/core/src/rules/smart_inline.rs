use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, BindingIdent, BlockStmtOrExpr, Callee, ComputedPropName, Decl, DoWhileStmt, Expr,
    ExprStmt, ForInStmt, ForOfStmt, ForStmt, Ident, ImportSpecifier, KeyValuePatProp, Lit,
    MemberExpr, MemberProp, Module, ModuleExportName, ModuleItem, Number, ObjectPat, ObjectPatProp,
    Pat, PropName, Stmt, VarDecl, VarDeclKind, VarDeclarator, WhileStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::is_stable_builtin_alias_root;

use super::decl_utils::same_ident;
use super::RewriteLevel;

pub struct SmartInline {
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: HashSet<BindingKey>,
    for_init_bindings: Vec<HashSet<BindingKey>>,
}

impl SmartInline {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            unresolved_mark: None,
            use_state_bindings: HashSet::new(),
            for_init_bindings: Vec::new(),
        }
    }

    pub fn new_with_mark(level: RewriteLevel, unresolved_mark: Mark) -> Self {
        Self {
            level,
            unresolved_mark: Some(unresolved_mark),
            use_state_bindings: HashSet::new(),
            for_init_bindings: Vec::new(),
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
            inline_module_builtin_aliases(module, self.unresolved_mark);
        }

        let context_for_init_bindings = self.context_for_init_bindings();
        process_module_stmt_runs(
            &mut module.body,
            self.level,
            self.unresolved_mark,
            &self.use_state_bindings,
            &context_for_init_bindings,
        );

        module.visit_mut_children_with(self);
        self.use_state_bindings = previous_use_state_bindings;
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        let taken = std::mem::take(stmts);
        let context_for_init_bindings = self.context_for_init_bindings();
        *stmts = process_stmts(
            taken,
            self.level,
            self.unresolved_mark,
            &self.use_state_bindings,
            &context_for_init_bindings,
        );
        stmts.visit_mut_children_with(self);
    }

    fn visit_mut_for_stmt(&mut self, stmt: &mut ForStmt) {
        stmt.init.visit_mut_with(self);
        stmt.test.visit_mut_with(self);
        stmt.update.visit_mut_with(self);

        self.for_init_bindings
            .push(collect_for_stmt_init_bindings(stmt));
        stmt.body.visit_mut_with(self);
        self.for_init_bindings.pop();
    }
}

impl SmartInline {
    fn context_for_init_bindings(&self) -> HashSet<BindingKey> {
        self.for_init_bindings
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
    context_for_init_bindings: &HashSet<BindingKey>,
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
                    context_for_init_bindings,
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
        context_for_init_bindings,
    );

    *body = new_body;
}

fn flush_stmt_run(
    new_body: &mut Vec<ModuleItem>,
    run: &mut Vec<Stmt>,
    level: RewriteLevel,
    unresolved_mark: Option<Mark>,
    use_state_bindings: &HashSet<BindingKey>,
    context_for_init_bindings: &HashSet<BindingKey>,
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
            context_for_init_bindings,
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
    context_for_init_bindings: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    // Pass 0: inline builtin global aliases (const x = Math.floor → replace x with Math.floor)
    // Standard+ only; this assumes globals and builtin properties are not patched
    // between alias capture and use.
    let stmts = if level >= RewriteLevel::Standard {
        inline_builtin_aliases_stmts(stmts, unresolved_mark)
    } else {
        stmts
    };
    if level < RewriteLevel::Standard {
        return stmts;
    }
    // Pass 1: inline single-use const declarations (temp vars)
    let stmts = inline_temp_vars(stmts, context_for_init_bindings);
    // Pass 1b: recover the React useState tuple pattern without making generic
    // numeric property reads iterable.
    let stmts = fold_use_state_tuple_reads(stmts, use_state_bindings);
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

/// Scope-aware key for arrow wrapper candidates: (symbol, SyntaxContext from resolver).
type BindingKey = (Atom, SyntaxContext);

#[derive(Default)]
struct GlobalUsageStats {
    callable_uses: usize,
    blocked_uses: usize,
}

/// Inline `const c = Object.defineProperty` → replace all `c(...)` with `Object.defineProperty(...)`.
/// Also handles bare builtin aliases like `const E = TypeError` and `const O = Object`.
/// These aliases are created by minifiers to save bytes and can be restored under
/// the standard+ `stable_builtins` assumption.
fn inline_module_builtin_aliases(module: &mut Module, unresolved_mark: Option<Mark>) {
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
        if is_builtin_alias_expr(init, unresolved_mark) {
            candidates.insert((bi.id.sym.clone(), bi.id.ctxt), init.clone());
        }
    }

    if candidates.is_empty() {
        return;
    }

    let usage_stats = collect_builtin_alias_usage_in_module(module, &candidates);
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, _)| {
            usage_stats
                .get(key)
                .is_some_and(|stats| stats.replaceable_uses > 0 && stats.blocked_uses == 0)
        })
        .collect();

    if to_inline.is_empty() {
        return;
    }

    // Remove definition stmts and replace all usages globally.
    module.body.retain(|item| {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if to_inline.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        return false;
                    }
                }
            }
        }
        true
    });

    let mut inliner = BuiltinAliasInliner { map: &to_inline };
    module.visit_mut_with(&mut inliner);
}

/// Same as `inline_module_builtin_aliases` but operates on a `Vec<Stmt>` (function bodies).
/// Handles `const Math_floor = Math.floor` inside nested scopes.
fn inline_builtin_aliases_stmts(mut stmts: Vec<Stmt>, unresolved_mark: Option<Mark>) -> Vec<Stmt> {
    let mut candidates: HashMap<BindingKey, Box<Expr>> = HashMap::new();
    for stmt in &stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
            continue;
        }
        let decl = &var.decls[0];
        let Pat::Ident(bi) = &decl.name else { continue };
        let Some(init) = &decl.init else { continue };
        if is_builtin_alias_expr(init, unresolved_mark) {
            candidates.insert((bi.id.sym.clone(), bi.id.ctxt), init.clone());
        }
    }

    if candidates.is_empty() {
        return stmts;
    }

    let usage_stats = collect_builtin_alias_usage_in_stmts(&stmts, &candidates);
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, _)| {
            usage_stats
                .get(key)
                .is_some_and(|stats| stats.replaceable_uses > 0 && stats.blocked_uses == 0)
        })
        .collect();

    if to_inline.is_empty() {
        return stmts;
    }

    // Remove definition stmts.
    stmts.retain(|stmt| {
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                if let Pat::Ident(bi) = &var.decls[0].name {
                    if to_inline.contains_key(&(bi.id.sym.clone(), bi.id.ctxt)) {
                        return false;
                    }
                }
            }
        }
        true
    });

    // Replace all usages
    let mut inliner = BuiltinAliasInliner { map: &to_inline };
    stmts.visit_mut_with(&mut inliner);
    stmts
}

fn is_builtin_alias_expr(expr: &Expr, unresolved_mark: Option<Mark>) -> bool {
    match expr {
        Expr::Ident(id) => is_unresolved_builtin_ident(id, unresolved_mark),
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(_),
            ..
        }) => {
            if let Expr::Ident(obj_id) = obj.as_ref() {
                is_unresolved_builtin_ident(obj_id, unresolved_mark)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn is_unresolved_builtin_ident(id: &Ident, unresolved_mark: Option<Mark>) -> bool {
    is_stable_builtin_alias_root(&id.sym)
        && unresolved_mark.is_none_or(|mark| id.ctxt.outer() == mark)
}

#[derive(Default)]
struct BuiltinAliasUsageStats {
    replaceable_uses: usize,
    blocked_uses: usize,
}

fn collect_builtin_alias_usage_in_module(
    module: &Module,
    candidates: &HashMap<BindingKey, Box<Expr>>,
) -> HashMap<BindingKey, BuiltinAliasUsageStats> {
    let mut stats: HashMap<BindingKey, BuiltinAliasUsageStats> = candidates
        .keys()
        .map(|key| (key.clone(), BuiltinAliasUsageStats::default()))
        .collect();

    for item in &module.body {
        if is_builtin_alias_definition_item(item, candidates) {
            continue;
        }
        let mut counter = BuiltinAliasUsageCounter { stats: &mut stats };
        item.visit_with(&mut counter);
    }

    stats
}

fn collect_builtin_alias_usage_in_stmts(
    stmts: &[Stmt],
    candidates: &HashMap<BindingKey, Box<Expr>>,
) -> HashMap<BindingKey, BuiltinAliasUsageStats> {
    let mut stats: HashMap<BindingKey, BuiltinAliasUsageStats> = candidates
        .keys()
        .map(|key| (key.clone(), BuiltinAliasUsageStats::default()))
        .collect();

    for stmt in stmts {
        if is_builtin_alias_definition_stmt(stmt, candidates) {
            continue;
        }
        let mut counter = BuiltinAliasUsageCounter { stats: &mut stats };
        stmt.visit_with(&mut counter);
    }

    stats
}

fn is_builtin_alias_definition_item(
    item: &ModuleItem,
    candidates: &HashMap<BindingKey, Box<Expr>>,
) -> bool {
    matches!(item, ModuleItem::Stmt(stmt) if is_builtin_alias_definition_stmt(stmt, candidates))
}

fn is_builtin_alias_definition_stmt(
    stmt: &Stmt,
    candidates: &HashMap<BindingKey, Box<Expr>>,
) -> bool {
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

struct BuiltinAliasUsageCounter<'a> {
    stats: &'a mut HashMap<BindingKey, BuiltinAliasUsageStats>,
}

impl Visit for BuiltinAliasUsageCounter<'_> {
    fn visit_new_expr(&mut self, new_expr: &swc_core::ecma::ast::NewExpr) {
        new_expr.callee.visit_with(self);
        new_expr.args.visit_with(self);
        new_expr.type_args.visit_with(self);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(id) = expr {
            if let Some(stats) = self.stats.get_mut(&(id.sym.clone(), id.ctxt)) {
                stats.replaceable_uses += 1;
                return;
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        if let Some(stats) = self.stats.get_mut(&(id.sym.clone(), id.ctxt)) {
            stats.blocked_uses += 1;
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, _: &PropName) {}
}

/// Replaces all ident usages with the builtin member expression, across all scopes.
struct BuiltinAliasInliner<'a> {
    map: &'a HashMap<BindingKey, Box<Expr>>,
}

impl VisitMut for BuiltinAliasInliner<'_> {
    fn visit_mut_new_expr(&mut self, new_expr: &mut swc_core::ecma::ast::NewExpr) {
        new_expr.callee.visit_mut_with(self);
        new_expr.args.visit_mut_with(self);
        new_expr.type_args.visit_mut_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        if let Expr::Ident(id) = expr {
            let key = (id.sym.clone(), id.ctxt);
            if let Some(replacement) = self.map.get(&key) {
                *expr = *replacement.clone();
            }
        }
    }
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
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
    context_for_init_bindings: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    // Collect candidates: `const t = e` where e is a simple expr.
    // Only inline if t is used exactly once in the rest of the block (not in nested functions)
    let mut candidates: HashMap<BindingKey, TempCandidate> = HashMap::new();

    for (idx, stmt) in stmts.iter().enumerate() {
        if let Stmt::Decl(Decl::Var(var)) = stmt {
            if var.kind == VarDeclKind::Const && var.decls.len() == 1 {
                let decl = &var.decls[0];
                if let Pat::Ident(bi) = &decl.name {
                    if let Some(init) = &decl.init {
                        if is_simple_expr(init) {
                            let key = (bi.id.sym.clone(), bi.id.ctxt);
                            candidates.insert(
                                key,
                                TempCandidate {
                                    init: init.clone(),
                                    def_idx: idx,
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

    let analysis = TempUsageAnalysis::collect(&stmts, &candidates, context_for_init_bindings);

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

fn stmt_has_top_level_side_effect(stmt: &Stmt) -> bool {
    use swc_core::ecma::ast::{
        AssignExpr, AssignTarget, AwaitExpr, CallExpr, NewExpr, SimpleAssignTarget, UnaryExpr,
        UpdateExpr,
    };

    struct SideEffectFinder {
        found: bool,
    }

    impl Visit for SideEffectFinder {
        fn visit_call_expr(&mut self, _: &CallExpr) {
            self.found = true;
        }

        fn visit_new_expr(&mut self, _: &NewExpr) {
            self.found = true;
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if !matches!(
                &assign.left,
                AssignTarget::Simple(SimpleAssignTarget::Ident(_))
            ) {
                self.found = true;
            }
            assign.right.visit_with(self);
        }

        fn visit_update_expr(&mut self, update: &UpdateExpr) {
            if !matches!(update.arg.as_ref(), Expr::Ident(_)) {
                self.found = true;
            }
        }

        fn visit_await_expr(&mut self, _: &AwaitExpr) {
            self.found = true;
        }

        fn visit_yield_expr(&mut self, _: &swc_core::ecma::ast::YieldExpr) {
            self.found = true;
        }

        fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
            if unary.op == swc_core::ecma::ast::UnaryOp::Delete {
                self.found = true;
            } else {
                unary.visit_children_with(self);
            }
        }

        fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}
        fn visit_arrow_expr(&mut self, _: &swc_core::ecma::ast::ArrowExpr) {}
        fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {}
    }

    let mut finder = SideEffectFinder { found: false };
    stmt.visit_with(&mut finder);
    finder.found
}

fn member_root_ident(member: &MemberExpr) -> Option<&Ident> {
    match member.obj.as_ref() {
        Expr::Ident(id) => Some(id),
        Expr::Member(member) => member_root_ident(member),
        _ => None,
    }
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
}

#[derive(Default)]
struct TempUsageInfo {
    ref_count: usize,
    use_idx: Option<usize>,
    used_above_decl: bool,
    used_in_loop: bool,
    used_in_nested_function: bool,
    source_mutated_after_def: bool,
}

impl TempUsageInfo {
    fn can_inline(&self, candidate: &TempCandidate, analysis: &TempUsageAnalysis) -> bool {
        if self.ref_count != 1
            || self.used_above_decl
            || self.used_in_nested_function
            || self.source_mutated_after_def
        {
            return false;
        }

        let Some(use_idx) = self.use_idx else {
            return false;
        };

        if let Expr::Ident(src_id) = candidate.init.as_ref() {
            let src_key = (src_id.sym.clone(), src_id.ctxt);
            let Some(source) = analysis.source_binding(&src_key) else {
                return !self.used_in_loop
                    && !analysis.has_side_effect_between(candidate.def_idx, use_idx);
            };

            if !source.is_safe_before(candidate.def_idx)
                || self.used_in_loop && !source.is_loop_stable()
            {
                return false;
            }
        } else if self.used_in_loop {
            return false;
        }

        if let Expr::Member(member) = candidate.init.as_ref() {
            if let Some(src_id) = member_root_ident(member) {
                let src_key = (src_id.sym.clone(), src_id.ctxt);
                if analysis.property_mutated_between(&src_key, candidate.def_idx, use_idx) {
                    return false;
                }
            }
        }

        !analysis.has_side_effect_between(candidate.def_idx, use_idx)
    }
}

struct TempUsageAnalysis {
    usage: HashMap<BindingKey, TempUsageInfo>,
    source_bindings: HashMap<BindingKey, SourceBindingInfo>,
    property_mutations: HashMap<BindingKey, Vec<usize>>,
    side_effect_stmts: Vec<usize>,
}

impl TempUsageAnalysis {
    fn collect(
        stmts: &[Stmt],
        candidates: &HashMap<BindingKey, TempCandidate>,
        context_for_init_bindings: &HashSet<BindingKey>,
    ) -> Self {
        let mut analysis = Self {
            usage: candidates
                .keys()
                .map(|key| (key.clone(), TempUsageInfo::default()))
                .collect(),
            source_bindings: context_for_init_bindings
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        SourceBindingInfo {
                            declared_in_for_init: true,
                            ..SourceBindingInfo::default()
                        },
                    )
                })
                .collect(),
            property_mutations: HashMap::new(),
            side_effect_stmts: Vec::new(),
        };

        let mut source_collector = SourceBindingCollector {
            source_bindings: &mut analysis.source_bindings,
            seen_refs: HashSet::new(),
            stmt_idx: 0,
            in_for_init: false,
            var_kind: None,
        };
        for (idx, stmt) in stmts.iter().enumerate() {
            source_collector.stmt_idx = idx;
            stmt.visit_with(&mut source_collector);
        }

        for (idx, stmt) in stmts.iter().enumerate() {
            if stmt_is_temp_definition(stmt, candidates) {
                continue;
            }

            if stmt_has_top_level_side_effect(stmt) {
                analysis.side_effect_stmts.push(idx);
            }

            let mut collector = TempUsageCollector {
                analysis: &mut analysis,
                candidates,
                stmt_idx: idx,
                loop_depth: 0,
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

    fn property_mutated_between(&self, key: &BindingKey, def_idx: usize, use_idx: usize) -> bool {
        self.property_mutations
            .get(key)
            .is_some_and(|indices| indices.iter().any(|idx| def_idx < *idx && *idx < use_idx))
    }

    fn has_side_effect_between(&self, def_idx: usize, use_idx: usize) -> bool {
        self.side_effect_stmts
            .iter()
            .any(|idx| def_idx < *idx && *idx < use_idx)
    }
}

#[derive(Default)]
struct SourceBindingInfo {
    decl_idx: Option<usize>,
    var_kind: Option<VarDeclKind>,
    declared_in_for_init: bool,
    used_above_decl: bool,
}

impl SourceBindingInfo {
    fn is_safe_before(&self, candidate_def_idx: usize) -> bool {
        !self.used_above_decl
            && !self.declared_in_for_init
            && self
                .decl_idx
                .is_none_or(|decl_idx| decl_idx <= candidate_def_idx)
    }

    fn is_loop_stable(&self) -> bool {
        self.var_kind == Some(VarDeclKind::Const)
            && !self.declared_in_for_init
            && !self.used_above_decl
    }
}

struct SourceBindingCollector<'a> {
    source_bindings: &'a mut HashMap<BindingKey, SourceBindingInfo>,
    seen_refs: HashSet<BindingKey>,
    stmt_idx: usize,
    in_for_init: bool,
    var_kind: Option<VarDeclKind>,
}

impl Visit for SourceBindingCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        self.seen_refs.insert((id.sym.clone(), id.ctxt));
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        decl.init.visit_with(self);

        let Some(binding) = direct_binding_ident_from_pat(&decl.name) else {
            return;
        };
        let key = (binding.id.sym.clone(), binding.id.ctxt);
        let info = self.source_bindings.entry(key.clone()).or_default();
        info.decl_idx.get_or_insert(self.stmt_idx);
        info.var_kind
            .get_or_insert(self.var_kind.unwrap_or(VarDeclKind::Var));
        info.declared_in_for_init |= self.in_for_init;
        info.used_above_decl |= self.seen_refs.contains(&key);
    }

    fn visit_var_decl(&mut self, var: &VarDecl) {
        let previous_var_kind = self.var_kind;
        self.var_kind = Some(var.kind);
        var.visit_children_with(self);
        self.var_kind = previous_var_kind;
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        let previous_in_for_init = self.in_for_init;
        self.in_for_init = true;
        stmt.init.visit_with(self);
        self.in_for_init = previous_in_for_init;

        stmt.test.visit_with(self);
        stmt.update.visit_with(self);
        stmt.body.visit_with(self);
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

fn collect_for_stmt_init_bindings(stmt: &ForStmt) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();
    let Some(swc_core::ecma::ast::VarDeclOrExpr::VarDecl(var)) = &stmt.init else {
        return bindings;
    };

    for decl in &var.decls {
        if let Some(binding) = direct_binding_ident_from_pat(&decl.name) {
            bindings.insert((binding.id.sym.clone(), binding.id.ctxt));
        }
    }

    bindings
}

struct TempUsageCollector<'a> {
    analysis: &'a mut TempUsageAnalysis,
    candidates: &'a HashMap<BindingKey, TempCandidate>,
    stmt_idx: usize,
    loop_depth: usize,
}

impl Visit for TempUsageCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let key = (id.sym.clone(), id.ctxt);
        if let Some(candidate) = self.candidates.get(&key) {
            if let Some(usage) = self.analysis.usage.get_mut(&key) {
                usage.ref_count += 1;
                usage.use_idx = Some(self.stmt_idx);
                usage.used_above_decl |= self.stmt_idx < candidate.def_idx;
                usage.used_in_loop |= self.loop_depth > 0;
            }
        }
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        use swc_core::ecma::ast::{AssignTarget, SimpleAssignTarget};

        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(id)) => {
                self.record_direct_mutation(&(id.sym.clone(), id.ctxt));
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.record_property_mutation(member);
            }
            _ => {}
        }

        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        match update.arg.as_ref() {
            Expr::Ident(id) => self.record_direct_mutation(&(id.sym.clone(), id.ctxt)),
            Expr::Member(member) => self.record_property_mutation(member),
            _ => {}
        }

        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &swc_core::ecma::ast::UnaryExpr) {
        if unary.op == swc_core::ecma::ast::UnaryOp::Delete {
            if let Expr::Member(member) = unary.arg.as_ref() {
                self.record_property_mutation(member);
            }
            return;
        }

        unary.visit_children_with(self);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let mut collector = NestedTempRefCollector {
            usage: &mut self.analysis.usage,
            candidates: self.candidates,
        };
        function.visit_children_with(&mut collector);
    }
    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let mut collector = NestedTempRefCollector {
            usage: &mut self.analysis.usage,
            candidates: self.candidates,
        };
        arrow.visit_children_with(&mut collector);
    }
    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        let mut collector = NestedTempRefCollector {
            usage: &mut self.analysis.usage,
            candidates: self.candidates,
        };
        class.visit_children_with(&mut collector);
    }
    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.init.visit_with(self);
        self.visit_within_loop(&stmt.test);
        self.visit_within_loop(&stmt.update);
        self.visit_within_loop(&stmt.body);
    }
    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        self.visit_within_loop(&stmt.body);
    }
    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        stmt.left.visit_with(self);
        stmt.right.visit_with(self);
        self.visit_within_loop(&stmt.body);
    }
    fn visit_while_stmt(&mut self, stmt: &WhileStmt) {
        self.visit_within_loop(&stmt.test);
        self.visit_within_loop(&stmt.body);
    }
    fn visit_do_while_stmt(&mut self, stmt: &DoWhileStmt) {
        self.visit_within_loop(&stmt.body);
        self.visit_within_loop(&stmt.test);
    }
    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
    fn visit_prop_name(&mut self, _: &PropName) {}
}

impl TempUsageCollector<'_> {
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

    fn record_property_mutation(&mut self, member: &MemberExpr) {
        let Some(root) = member_root_ident(member) else {
            return;
        };
        self.analysis
            .property_mutations
            .entry((root.sym.clone(), root.ctxt))
            .or_default()
            .push(self.stmt_idx);
    }

    fn visit_within_loop<N>(&mut self, node: &N)
    where
        N: VisitWith<Self>,
    {
        self.loop_depth += 1;
        node.visit_with(self);
        self.loop_depth -= 1;
    }
}

struct NestedTempRefCollector<'a> {
    usage: &'a mut HashMap<BindingKey, TempUsageInfo>,
    candidates: &'a HashMap<BindingKey, TempCandidate>,
}

impl Visit for NestedTempRefCollector<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        let key = (id.sym.clone(), id.ctxt);
        if self.candidates.contains_key(&key) {
            if let Some(usage) = self.usage.get_mut(&key) {
                usage.used_in_nested_function = true;
            }
        }
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
                *expr = *replacement.clone();
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
    },
    /// obj[n] — maps to (binding_name, index)
    Index {
        binding: Option<BindingIdent>,
        index: usize,
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
    let mut result = Vec::with_capacity(stmts.len());
    let mut i = 0;

    while i < stmts.len() {
        if let Some(stmt) = try_fold_use_state_tuple_at(&stmts, i, use_state_bindings) {
            result.push(stmt);
            i += 3;
        } else {
            result.push(stmts[i].clone());
            i += 1;
        }
    }

    result
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

    Some(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
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

fn group_destructuring(stmts: Vec<Stmt>, level: RewriteLevel) -> Vec<Stmt> {
    // Scan for groups of consecutive `const t = obj.prop` / `const t = obj[n]`
    // where `obj` is a plain identifier.
    // Group by the obj name, emit destructuring when group is "flushed".

    let mut result: Vec<Stmt> = Vec::new();
    let mut current_obj: Option<(Ident, Vec<AccessKind>)> = None;
    let mut i = 0;
    let stmts_count = stmts.len();

    while i < stmts_count {
        let stmt = &stmts[i];

        let next_access = try_extract_prop_access(stmt)
            .map(|(obj, key, binding)| {
                (
                    obj,
                    AccessKind::Property {
                        binding,
                        prop_key: key,
                    },
                )
            })
            .or_else(|| {
                try_extract_index_access(stmt)
                    .map(|(obj, index, binding)| (obj, AccessKind::Index { binding, index }))
            });

        if let Some((obj_name, access)) = next_access {
            // Don't group built-in globals — `Object.defineProperty(...)` is clearer
            // than `defineProperty(...)` and destructuring can break `this` binding.
            if is_stable_builtin_alias_root(&obj_name.sym) {
                if let Some((obj, acc)) = current_obj.take() {
                    flush_group(&mut result, obj, acc, level);
                }
                result.push(stmts[i].clone());
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
        result.push(stmts[i].clone());
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
    // Build ObjectPat
    let mut props: Vec<ObjectPatProp> = Vec::new();

    for acc in &accesses {
        let AccessKind::Property { binding, prop_key } = acc else {
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
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
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
        let AccessKind::Index { binding, index } = acc else {
            continue;
        };
        if let Some(alias) = binding {
            elems[*index] = Some(Pat::Ident(alias.clone()));
        }
    }

    result.push(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
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

fn acc_to_stmt(obj: &Ident, acc: AccessKind) -> Stmt {
    match acc {
        AccessKind::Property { binding, prop_key } => {
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
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(obj.clone())),
                prop,
            });
            match binding {
                None => Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: Box::new(member_expr),
                }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: DUMMY_SP,
                        name: Pat::Ident(alias),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
        AccessKind::Index { binding, index } => {
            let member_expr = Expr::Member(MemberExpr {
                span: DUMMY_SP,
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
                    span: DUMMY_SP,
                    expr: Box::new(member_expr),
                }),
                Some(alias) => Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Const,
                    declare: false,
                    decls: vec![VarDeclarator {
                        span: DUMMY_SP,
                        name: Pat::Ident(alias),
                        init: Some(Box::new(member_expr)),
                        definite: false,
                    }],
                }))),
            }
        }
    }
}
