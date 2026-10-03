use crate::collections::{HashMap, HashSet};

use swc_core::common::{Mark, Span};
use swc_core::ecma::ast::{
    Decl, ExportSpecifier, Expr, Ident, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, Pat, PropName, Stmt, UnaryExpr, UnaryOp, UpdateExpr, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::eval_utils::has_dynamic_scope_construct;
use super::helper_matcher::{
    binding_key, collect_refs, remove_var_declarators_by_binding, var_declarator_binding_key,
    BindingKey,
};
use crate::js_names::is_stable_builtin_alias_root;
use crate::utils::paren::strip_parens;

/// Inlining an alias reads the builtin (`Object`, `Math`) as the global at
/// every former use site. A `with` statement or a direct eval in the same
/// scope can bind that name at runtime, so both entry points skip when either
/// construct is present, whatever the declaration kind
/// (docs/rewrite-assumptions.md, dynamic-scope skip).
#[derive(Clone, Copy)]
pub(crate) struct BuiltinAliasInlineOptions {
    allow_var: bool,
    require_no_var_use_before_decl: bool,
}

impl BuiltinAliasInlineOptions {
    pub(crate) const fn const_only() -> Self {
        Self {
            allow_var: false,
            require_no_var_use_before_decl: false,
        }
    }

    pub(crate) const fn early_var_aliases() -> Self {
        Self {
            allow_var: true,
            require_no_var_use_before_decl: true,
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

/// `pinned` bindings keep their declaration, in addition to aliases named by
/// an `export { alias }` specifier.
pub(crate) fn inline_module_builtin_aliases(
    module: &mut Module,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
    pinned: &HashSet<BindingKey>,
) -> bool {
    let mut candidates = collect_module_candidates(module, unresolved_mark, options);
    if candidates.is_empty() {
        return false;
    }

    // `export { alias }` names the binding itself: the specifier cannot carry
    // `Object.create`, so removing the declaration would leave it dangling and
    // the module would fail to link. Keep exported aliases as they are.
    let exported = collect_local_export_specifier_keys(module);
    candidates.retain(|key, _| !exported.contains(key) && !pinned.contains(key));
    if candidates.is_empty() {
        return false;
    }

    if has_dynamic_scope_construct(module) {
        return false;
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

/// `pinned` bindings keep their declaration: module-scope aliases named by an
/// `export { alias }` specifier (see `collect_local_export_specifier_keys`).
pub(crate) fn inline_builtin_aliases_stmts(
    mut stmts: Vec<Stmt>,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
    pinned: &HashSet<BindingKey>,
) -> Vec<Stmt> {
    let mut candidates = collect_stmt_candidates(&stmts, unresolved_mark, options);
    candidates.retain(|key, _| !pinned.contains(key));
    if candidates.is_empty() {
        return stmts;
    }

    if has_dynamic_scope_construct(stmts.as_slice()) {
        return stmts;
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

/// The local bindings named by `export { local }` / `export { local as x }`
/// specifiers without a source.
pub(crate) fn collect_local_export_specifier_keys(module: &Module) -> HashSet<BindingKey> {
    let mut keys = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) = item else {
            continue;
        };
        if export.src.is_some() {
            continue;
        }
        for specifier in &export.specifiers {
            let ExportSpecifier::Named(named) = specifier else {
                continue;
            };
            if let ModuleExportName::Ident(local) = &named.orig {
                keys.insert(binding_key(local));
            }
        }
    }
    keys
}

fn collect_module_candidates(
    module: &Module,
    unresolved_mark: Option<Mark>,
    options: BuiltinAliasInlineOptions,
) -> HashMap<BindingKey, BuiltinAliasCandidate> {
    let mut candidates = HashMap::default();
    let mut seen_single_decl_keys = HashSet::default();
    let mut duplicate_keys = HashSet::default();

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
            &mut seen_single_decl_keys,
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
    let mut candidates = HashMap::default();
    let mut seen_single_decl_keys = HashSet::default();
    let mut duplicate_keys = HashSet::default();

    for (def_index, stmt) in stmts.iter().enumerate() {
        collect_candidate_from_stmt(
            stmt,
            def_index,
            unresolved_mark,
            options,
            &mut candidates,
            &mut seen_single_decl_keys,
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
    seen_single_decl_keys: &mut HashSet<BindingKey>,
    duplicate_keys: &mut HashSet<BindingKey>,
) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    if var.decls.len() != 1 {
        return;
    }

    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return;
    };
    let key = binding_key(&binding.id);
    // Any same-key single-declarator var — alias-shaped or not — blocks the
    // candidate: the definition matcher and declarator removal both match by
    // binding key alone, so a var redeclaration would be skipped during usage
    // counting and deleted along with the alias.
    if !seen_single_decl_keys.insert(key.clone()) {
        duplicate_keys.insert(key.clone());
    }

    if var.kind != VarDeclKind::Const && !(options.allow_var && var.kind == VarDeclKind::Var) {
        return;
    }
    let Some(init) = &decl.init else {
        return;
    };
    if !is_builtin_alias_expr(init, unresolved_mark) {
        return;
    }

    candidates.insert(
        key,
        BuiltinAliasCandidate {
            init: init.clone(),
            decl_kind: var.kind,
            def_index,
        },
    );
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
    let targets = HashSet::from_iter([key.clone()]);
    module
        .body
        .iter()
        .take(index)
        .any(|item| !collect_refs(item, &targets).is_empty())
}

fn stmts_have_ref_before_index(stmts: &[Stmt], key: &BindingKey, index: usize) -> bool {
    let targets = HashSet::from_iter([key.clone()]);
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

    // `e++` / `--e` / `delete e` mutate the binding but reach the counter as
    // plain `Expr::Ident` args, which `visit_expr` would count as replaceable.
    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(id) = strip_parens(&update.arg) {
            if let Some(stats) = self.stats.get_mut(&(id.sym.clone(), id.ctxt)) {
                stats.blocked_uses += 1;
                return;
            }
        }
        update.visit_children_with(self);
    }

    fn visit_unary_expr(&mut self, unary: &UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            if let Expr::Ident(id) = strip_parens(&unary.arg) {
                if let Some(stats) = self.stats.get_mut(&(id.sym.clone(), id.ctxt)) {
                    stats.blocked_uses += 1;
                    return;
                }
            }
        }
        unary.visit_children_with(self);
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }
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

    fn visit_mut_prop_name(&mut self, prop: &mut PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_mut_with(self);
        }
    }
}

fn set_expr_span(expr: &mut Expr, span: Span) {
    match expr {
        Expr::Ident(id) => id.span = span,
        Expr::Member(member) => member.span = span,
        _ => {}
    }
}
