use std::collections::{HashMap, HashSet};

use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{
    CallExpr, Decl, Expr, Ident, MemberExpr, MemberProp, Module, ModuleItem, Pat, PropName, Stmt,
    VarDeclKind, WithStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::eval_utils::is_direct_eval_call;
use super::helper_matcher::{
    binding_key, collect_refs, remove_var_declarators_by_binding, var_declarator_binding_key,
    BindingKey,
};
use crate::js_names::is_stable_builtin_alias_root;

#[derive(Clone, Copy)]
pub(crate) struct BuiltinAliasInlineOptions {
    allow_var: bool,
    require_no_var_use_before_decl: bool,
    reject_var_with_dynamic_scope: bool,
}

impl BuiltinAliasInlineOptions {
    pub(crate) const fn const_only() -> Self {
        Self {
            allow_var: false,
            require_no_var_use_before_decl: false,
            reject_var_with_dynamic_scope: false,
        }
    }

    pub(crate) const fn early_var_aliases() -> Self {
        Self {
            allow_var: true,
            require_no_var_use_before_decl: true,
            reject_var_with_dynamic_scope: true,
        }
    }
}

struct BuiltinAliasCandidate {
    init: Box<Expr>,
    decl_kind: VarDeclKind,
    def_index: usize,
}

#[derive(Default)]
struct BuiltinAliasUsageStats {
    replaceable_uses: usize,
    blocked_uses: usize,
}

pub(crate) fn inline_module_builtin_aliases(
    module: &mut Module,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
) -> bool {
    let mut candidates = collect_module_candidates(module, unresolved_mark, options);
    if candidates.is_empty() {
        return false;
    }

    if options.reject_var_with_dynamic_scope && module_has_dynamic_scope_construct(module) {
        candidates.retain(|_, candidate| candidate.decl_kind != VarDeclKind::Var);
    }

    if options.require_no_var_use_before_decl {
        candidates.retain(|key, candidate| {
            candidate.decl_kind != VarDeclKind::Var
                || !module_has_ref_before_index(module, key, candidate.def_index)
        });
    }

    if candidates.is_empty() {
        return false;
    }

    let usage_stats = collect_builtin_alias_usage_in_module(module, &candidates);
    let to_inline: HashMap<BindingKey, Box<Expr>> = candidates
        .into_iter()
        .filter(|(key, _)| {
            usage_stats
                .get(key)
                .is_some_and(|stats| stats.replaceable_uses > 0 && stats.blocked_uses == 0)
        })
        .map(|(key, candidate)| (key, candidate.init))
        .collect();

    if to_inline.is_empty() {
        return false;
    }

    let removable = to_inline.keys().cloned().collect();
    remove_var_declarators_by_binding(&mut module.body, &removable);

    let mut inliner = BuiltinAliasInliner { map: &to_inline };
    module.visit_mut_with(&mut inliner);
    true
}

pub(crate) fn inline_builtin_aliases_stmts(
    mut stmts: Vec<Stmt>,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
) -> Vec<Stmt> {
    let mut candidates = collect_stmt_candidates(&stmts, unresolved_mark, options);
    if candidates.is_empty() {
        return stmts;
    }

    if options.reject_var_with_dynamic_scope && stmts_have_dynamic_scope_construct(&stmts) {
        candidates.retain(|_, candidate| candidate.decl_kind != VarDeclKind::Var);
    }

    if options.require_no_var_use_before_decl {
        candidates.retain(|key, candidate| {
            candidate.decl_kind != VarDeclKind::Var
                || !stmts_have_ref_before_index(&stmts, key, candidate.def_index)
        });
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
        .map(|(key, candidate)| (key, candidate.init))
        .collect();

    if to_inline.is_empty() {
        return stmts;
    }

    stmts.retain(|stmt| !is_builtin_alias_definition_stmt(stmt, &to_inline));

    let mut inliner = BuiltinAliasInliner { map: &to_inline };
    stmts.visit_mut_with(&mut inliner);
    stmts
}

fn collect_module_candidates(
    module: &Module,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
) -> HashMap<BindingKey, BuiltinAliasCandidate> {
    let mut candidates = HashMap::new();
    let mut duplicate_keys = HashSet::new();

    for (def_index, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(stmt) = item else {
            continue;
        };
        collect_candidate_from_stmt(
            stmt,
            def_index,
            unresolved_mark,
            options,
            &mut candidates,
            &mut duplicate_keys,
        );
    }

    for key in duplicate_keys {
        candidates.remove(&key);
    }
    candidates
}

fn collect_stmt_candidates(
    stmts: &[Stmt],
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
) -> HashMap<BindingKey, BuiltinAliasCandidate> {
    let mut candidates = HashMap::new();
    let mut duplicate_keys = HashSet::new();

    for (def_index, stmt) in stmts.iter().enumerate() {
        collect_candidate_from_stmt(
            stmt,
            def_index,
            unresolved_mark,
            options,
            &mut candidates,
            &mut duplicate_keys,
        );
    }

    for key in duplicate_keys {
        candidates.remove(&key);
    }
    candidates
}

fn collect_candidate_from_stmt(
    stmt: &Stmt,
    def_index: usize,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
    candidates: &mut HashMap<BindingKey, BuiltinAliasCandidate>,
    duplicate_keys: &mut HashSet<BindingKey>,
) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    if var.decls.len() != 1 {
        return;
    }
    if var.kind != VarDeclKind::Const && !(options.allow_var && var.kind == VarDeclKind::Var) {
        return;
    }

    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return;
    };
    let Some(init) = &decl.init else {
        return;
    };
    if !is_builtin_alias_expr(init, unresolved_mark) {
        return;
    }

    let key = binding_key(&binding.id);
    if candidates
        .insert(
            key.clone(),
            BuiltinAliasCandidate {
                init: init.clone(),
                decl_kind: var.kind,
                def_index,
            },
        )
        .is_some()
    {
        duplicate_keys.insert(key);
    }
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

fn collect_builtin_alias_usage_in_module(
    module: &Module,
    candidates: &HashMap<BindingKey, BuiltinAliasCandidate>,
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
    candidates: &HashMap<BindingKey, BuiltinAliasCandidate>,
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

fn is_builtin_alias_definition_item<T>(
    item: &ModuleItem,
    candidates: &HashMap<BindingKey, T>,
) -> bool {
    matches!(item, ModuleItem::Stmt(stmt) if is_builtin_alias_definition_stmt(stmt, candidates))
}

fn is_builtin_alias_definition_stmt<T>(stmt: &Stmt, candidates: &HashMap<BindingKey, T>) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Some(key) = var_declarator_binding_key(&var.decls[0]) else {
        return false;
    };
    candidates.contains_key(&key)
}

fn module_has_ref_before_index(module: &Module, key: &BindingKey, index: usize) -> bool {
    let targets = HashSet::from([key.clone()]);
    module
        .body
        .iter()
        .take(index)
        .any(|item| !collect_refs(item, &targets).is_empty())
}

fn stmts_have_ref_before_index(stmts: &[Stmt], key: &BindingKey, index: usize) -> bool {
    let targets = HashSet::from([key.clone()]);
    stmts
        .iter()
        .take(index)
        .any(|stmt| !collect_refs(stmt, &targets).is_empty())
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
                let original_span = id.span;
                *expr = *replacement.clone();
                set_expr_span(expr, original_span);
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

fn set_expr_span(expr: &mut Expr, span: Span) {
    match expr {
        Expr::Ident(id) => id.span = span,
        Expr::Member(member) => member.span = span,
        _ => {}
    }
}

fn module_has_dynamic_scope_construct(module: &Module) -> bool {
    let mut visitor = DynamicScopeConstructFinder::default();
    module.visit_with(&mut visitor);
    visitor.found
}

fn stmts_have_dynamic_scope_construct(stmts: &[Stmt]) -> bool {
    let mut visitor = DynamicScopeConstructFinder::default();
    stmts.visit_with(&mut visitor);
    visitor.found
}

#[derive(Default)]
struct DynamicScopeConstructFinder {
    found: bool,
}

impl Visit for DynamicScopeConstructFinder {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if is_direct_eval_call(call) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_with_stmt(&mut self, _: &WithStmt) {
        self.found = true;
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if !self.found {
            expr.visit_children_with(self);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        if !self.found {
            stmt.visit_children_with(self);
        }
    }

    fn visit_module_item(&mut self, item: &ModuleItem) {
        if !self.found {
            item.visit_children_with(self);
        }
    }
}
