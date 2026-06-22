use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Decl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Expr, Module, ModuleDecl,
    ModuleExportName, ModuleItem, NamedExport, Pat, Prop, PropName, PropOrSpread, Stmt, VarDecl,
    VarDeclKind,
};
use swc_core::ecma::visit::VisitMut;

use crate::js_names::is_reserved_binding_name;

use super::rename_utils::{
    collect_module_names, collect_top_level_binding_infos, rename_bindings_in_module,
    rename_causes_shadowing, BindingId, BindingRename, TopLevelBindingInfo, TopLevelBindingKind,
};

pub struct UnExportRename;

#[derive(Clone)]
struct ExportRenamePlan {
    old: BindingId,
    old_name: Atom,
    new_name: Atom,
    /// If the export used aliases (var h = p), these are the alias names so we can
    /// match the export specifier's orig for cleanup.
    alias_names: Vec<Atom>,
    /// True when the rename source is a getter namespace (Pattern C).
    /// These bindings should not be promoted to export declarations because
    /// they're already exported via `export { name }` specifiers.
    from_getter_namespace: bool,
}

impl VisitMut for UnExportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let module_names = collect_module_names(module);
        let binding_infos = collect_top_level_binding_infos(module);
        let plans = collect_export_rename_plans(module, &module_names, &binding_infos);

        if plans.is_empty() {
            return;
        }

        let promotion_plans: Vec<_> = plans
            .iter()
            .filter(|p| !p.from_getter_namespace)
            .cloned()
            .collect();
        promote_renamed_bindings(module, &promotion_plans, &binding_infos);
        rewrite_export_aliases(module, &plans);

        let mut renames: Vec<BindingRename> = plans
            .iter()
            .map(|plan| BindingRename {
                old: plan.old.clone(),
                new: plan.new_name.clone(),
            })
            .collect();

        // Also rename alias bindings (e.g. `var h = p` — rename `h` → new_name
        // so remaining `h` references don't dangle after the alias decl is removed)
        for plan in &plans {
            for alias in &plan.alias_names {
                if let Some(alias_info) = binding_infos.get(alias) {
                    renames.push(BindingRename {
                        old: alias_info.id.clone(),
                        new: plan.new_name.clone(),
                    });
                }
            }
        }

        rename_bindings_in_module(module, &renames);
    }
}

fn collect_export_rename_plans(
    module: &Module,
    module_names: &std::collections::HashSet<Atom>,
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
) -> Vec<ExportRenamePlan> {
    // Compute which names will be freed by export renames.  Given
    //   export { i as x };  export { x as f };
    // the name `x` is occupied but will be freed because `x` is itself renamed
    // to `f`.  We pre-compute the full set of freed names so all renames can be
    // planned in a single pass without iterative chain-following.
    let freed_names = compute_freed_names(module, binding_infos, module_names);

    let mut plans = Vec::new();

    let mut changed = true;
    while changed {
        changed = false;

        for item in &module.body {
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) = item
            {
                if var.decls.len() == 1 {
                    if let (Pat::Ident(id), Some(init)) = (&var.decls[0].name, &var.decls[0].init) {
                        if let Expr::Ident(init_id) = init.as_ref() {
                            let Some(info) = binding_infos.get(&init_id.sym) else {
                                continue;
                            };
                            if info.id != (init_id.sym.clone(), init_id.ctxt) || info.exported {
                                continue;
                            }
                            let new_name = id.id.sym.clone();
                            if new_name != info.id.0
                                && !is_reserved_binding_name(&new_name)
                                && !name_is_import_binding(&new_name, module_names, binding_infos)
                                && !plans
                                    .iter()
                                    .any(|plan: &ExportRenamePlan| plan.old == info.id)
                                && !target_name_already_planned(&plans, &new_name, &info.id)
                                && !rename_causes_shadowing(module, &info.id, &new_name)
                            {
                                plans.push(ExportRenamePlan {
                                    old: info.id.clone(),
                                    old_name: info.id.0.clone(),
                                    new_name,
                                    alias_names: Vec::new(),
                                    from_getter_namespace: false,
                                });
                                changed = true;
                            }
                        }
                    }
                }
            }

            if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
                specifiers,
                src: None,
                ..
            })) = item
            {
                for specifier in specifiers {
                    let ExportSpecifier::Named(ExportNamedSpecifier {
                        orig,
                        exported: Some(exported),
                        ..
                    }) = specifier
                    else {
                        continue;
                    };
                    let orig_name = match orig {
                        ModuleExportName::Ident(ident) => ident.sym.clone(),
                        _ => continue,
                    };
                    let Some(orig_info) = binding_infos.get(&orig_name) else {
                        continue;
                    };
                    if orig_info.exported {
                        continue;
                    }
                    // Resolve through var aliases: if orig is `var h = p`,
                    // use `p`'s binding info so the real class/fn gets renamed.
                    let (info, old_name, alias_names) =
                        resolve_to_real_binding(orig_info, &orig_name, module, binding_infos);
                    let new_name = match exported {
                        ModuleExportName::Ident(ident) => ident.sym.clone(),
                        _ => continue,
                    };
                    if old_name == new_name
                        || new_name.len() < old_name.len()
                        || is_reserved_binding_name(&new_name)
                        || name_is_import_binding(&new_name, module_names, binding_infos)
                        || name_conflicts_with_unmoved_binding(
                            binding_infos,
                            &plans,
                            &new_name,
                            &freed_names,
                        )
                        || target_name_already_planned(&plans, &new_name, &info.id)
                        || plans
                            .iter()
                            .any(|plan: &ExportRenamePlan| plan.old == info.id)
                        || rename_causes_shadowing(module, &info.id, &new_name)
                    {
                        continue;
                    }
                    plans.push(ExportRenamePlan {
                        old: info.id.clone(),
                        old_name,
                        new_name,
                        alias_names,
                        from_getter_namespace: false,
                    });
                    changed = true;
                }
            }

            // Pattern C: exported getter namespace object.
            // `export const ns = { get subprocessEnv() { return Ym; }, ... }`
            // Each getter whose body is `return <ident>` provides a rename hint:
            // Ym → subprocessEnv. Only for non-exported bindings — exported ones
            // require cross-module coordination that this rule can't do.
            if let Some(getters) = extract_exported_getter_namespace(item) {
                for (getter_name, local_id) in getters {
                    let Some(info) = binding_infos.get(&local_id.0) else {
                        continue;
                    };
                    if info.id != local_id || info.exported {
                        continue;
                    }
                    if getter_name == info.id.0
                        || getter_name.len() < info.id.0.len()
                        || is_reserved_binding_name(&getter_name)
                        || name_is_import_binding(&getter_name, module_names, binding_infos)
                        || name_conflicts_with_unmoved_binding(
                            binding_infos,
                            &plans,
                            &getter_name,
                            &freed_names,
                        )
                        || target_name_already_planned(&plans, &getter_name, &info.id)
                        || plans
                            .iter()
                            .any(|plan: &ExportRenamePlan| plan.old == info.id)
                        || rename_causes_shadowing(module, &info.id, &getter_name)
                    {
                        continue;
                    }
                    plans.push(ExportRenamePlan {
                        old: info.id.clone(),
                        old_name: info.id.0.clone(),
                        new_name: getter_name,
                        alias_names: Vec::new(),
                        from_getter_namespace: true,
                    });
                    changed = true;
                }
            }
        }
    }

    plans
}

/// Compute the set of binding names that will be freed by export renames.
///
/// A name is "freed" if it appears as `orig` in `export { orig as exported }`
/// and the rename chain from `orig` terminates at a name that is either not a
/// binding or is itself freed (non-cyclically).
///
/// Example: `export { i as x }; export { x as f };` with bindings `i`, `x`
/// but no binding `f`.  Chain: x→f (free).  So `x` is freed, then `i→x` is
/// also safe.
fn compute_freed_names(
    module: &Module,
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
    module_names: &HashSet<Atom>,
) -> HashSet<Atom> {
    // Step 1: collect eligible rename edges (orig → exported).
    // Only include edges that the planner would actually accept: the exported
    // name must not be shorter than the orig, and the orig binding must not
    // already be an `export` declaration.
    let mut rename_edges: HashMap<Atom, Atom> = HashMap::new();
    for item in &module.body {
        if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
            specifiers,
            src: None,
            ..
        })) = item
        {
            for spec in specifiers {
                if let ExportSpecifier::Named(ExportNamedSpecifier {
                    orig: ModuleExportName::Ident(orig),
                    exported: Some(ModuleExportName::Ident(exported)),
                    ..
                }) = spec
                {
                    if orig.sym == exported.sym || exported.sym.len() < orig.sym.len() {
                        continue;
                    }
                    let Some(info) = binding_infos.get(&orig.sym) else {
                        continue;
                    };
                    if info.exported {
                        continue;
                    }
                    if is_reserved_binding_name(&exported.sym) {
                        continue;
                    }
                    if name_is_import_binding(&exported.sym, module_names, binding_infos) {
                        continue;
                    }
                    if rename_causes_shadowing(module, &info.id, &exported.sym) {
                        continue;
                    }
                    rename_edges.insert(orig.sym.clone(), exported.sym.clone());
                }
            }
        }
    }

    // Step 1b: remove edges whose target is claimed by multiple sources.
    // The planner will reject all but one via `target_name_already_planned`,
    // but we can't predict which wins, so conservatively drop all of them.
    let mut target_counts: HashMap<Atom, usize> = HashMap::new();
    for target in rename_edges.values() {
        *target_counts.entry(target.clone()).or_default() += 1;
    }
    rename_edges.retain(|_, target| target_counts.get(target).copied().unwrap_or(0) <= 1);

    // Step 2: for each edge, follow the chain to see if it terminates at a free
    // name.  Mark all names along successful chains as freed.
    let mut freed = HashSet::new();
    for start in rename_edges.keys() {
        if freed.contains(start) {
            continue;
        }
        // Walk the chain, collecting visited names
        let mut chain = Vec::new();
        let mut cursor = start;
        let mut visited = HashSet::new();
        let terminates_free = loop {
            if !visited.insert(cursor.clone()) {
                break false; // cycle
            }
            chain.push(cursor.clone());
            match rename_edges.get(cursor) {
                Some(next) => {
                    if !binding_infos.contains_key(next) || freed.contains(next) {
                        break true; // chain reaches a free name
                    }
                    cursor = next;
                }
                None => break false, // occupant is not being renamed away
            }
        };
        if terminates_free {
            freed.extend(chain);
        }
    }

    freed
}

fn name_is_import_binding(
    new_name: &Atom,
    module_names: &HashSet<Atom>,
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
) -> bool {
    module_names.contains(new_name) && !binding_infos.contains_key(new_name)
}

fn name_conflicts_with_unmoved_binding(
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
    plans: &[ExportRenamePlan],
    new_name: &Atom,
    freed_names: &HashSet<Atom>,
) -> bool {
    let Some(existing) = binding_infos.get(new_name) else {
        return false;
    };

    if plans.iter().any(|plan| plan.old == existing.id) {
        return false;
    }

    if freed_names.contains(new_name) {
        return false;
    }

    true
}

fn target_name_already_planned(
    plans: &[ExportRenamePlan],
    new_name: &Atom,
    old: &BindingId,
) -> bool {
    plans
        .iter()
        .any(|plan| plan.new_name == *new_name && plan.old != *old)
}

fn promote_renamed_bindings(
    module: &mut Module,
    plans: &[ExportRenamePlan],
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
) {
    let plans_by_item: HashMap<usize, &ExportRenamePlan> = plans
        .iter()
        .filter_map(|plan| {
            binding_infos
                .get(&plan.old_name)
                .map(|info| (info.item_index, plan))
        })
        .collect();

    let infos_by_old: HashMap<BindingId, &TopLevelBindingInfo> = binding_infos
        .values()
        .map(|info| (info.id.clone(), info))
        .collect();

    let mut new_body = Vec::with_capacity(module.body.len());
    for (item_index, item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        let Some(plan) = plans_by_item.get(&item_index).copied() else {
            new_body.push(item);
            continue;
        };
        let Some(info) = infos_by_old.get(&plan.old).copied() else {
            new_body.push(item);
            continue;
        };
        new_body.extend(rewrite_promoted_item(item, info));
    }
    module.body = new_body;
}

fn rewrite_promoted_item(item: ModuleItem, info: &TopLevelBindingInfo) -> Vec<ModuleItem> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => rewrite_var_decl(*var, false, info)
            .unwrap_or_else(|var| vec![ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(var))))]),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(var),
            ..
        })) => {
            rewrite_var_decl(*var, true, info).unwrap_or_else(|var| vec![wrap_var_decl(var, true)])
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
            vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: DUMMY_SP,
                decl: Decl::Fn(function),
            }))]
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(class))) => {
            vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: DUMMY_SP,
                decl: Decl::Class(class),
            }))]
        }
        other => vec![other],
    }
}

fn rewrite_var_decl(
    mut var: VarDecl,
    exported: bool,
    info: &TopLevelBindingInfo,
) -> Result<Vec<ModuleItem>, VarDecl> {
    let TopLevelBindingKind::Var { declarator_index } = info.kind else {
        return Err(var);
    };
    if declarator_index >= var.decls.len() {
        return Err(var);
    }

    let target = var.decls[declarator_index].clone();
    let Pat::Ident(_) = &target.name else {
        return Err(var);
    };

    let before = var.decls[..declarator_index].to_vec();
    let after = var.decls[declarator_index + 1..].to_vec();

    let mut items = Vec::new();
    if !before.is_empty() {
        let mut before_decl = var.clone();
        before_decl.decls = before;
        items.push(wrap_var_decl(before_decl, exported));
    }

    let mut promoted_decl = var.clone();
    promoted_decl.decls = vec![target];
    items.push(wrap_var_decl(promoted_decl, true));

    if !after.is_empty() {
        var.decls = after;
        items.push(wrap_var_decl(var, exported));
    }

    Ok(items)
}

fn wrap_var_decl(var: VarDecl, exported: bool) -> ModuleItem {
    if exported {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            span: DUMMY_SP,
            decl: Decl::Var(Box::new(var)),
        }))
    } else {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(var))))
    }
}

fn rewrite_export_aliases(module: &mut Module, plans: &[ExportRenamePlan]) {
    let plans_by_old_name: HashMap<Atom, &ExportRenamePlan> = plans
        .iter()
        .map(|plan| (plan.old_name.clone(), plan))
        .collect();
    let plans_by_old: HashMap<BindingId, &ExportRenamePlan> =
        plans.iter().map(|plan| (plan.old.clone(), plan)).collect();
    // Also index by alias name so we can match export specifiers that
    // reference the alias (e.g. `export { h as Foo }` where h was resolved to p).
    let plans_by_alias: HashMap<Atom, &ExportRenamePlan> = plans
        .iter()
        .flat_map(|plan| {
            plan.alias_names
                .iter()
                .map(move |alias| (alias.clone(), plan))
        })
        .collect();

    // Collect alias var names to remove (var h = p where h was the alias)
    let alias_var_names: std::collections::HashSet<Atom> = plans
        .iter()
        .flat_map(|plan| plan.alias_names.iter().cloned())
        .collect();

    let mut new_body = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        // Remove alias var declarations (var h = p)
        if !alias_var_names.is_empty() {
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var))) = item {
                if var.decls.len() == 1 {
                    if let Pat::Ident(binding) = &var.decls[0].name {
                        if alias_var_names.contains(&binding.id.sym) {
                            continue;
                        }
                    }
                }
            }
        }
        if let Some(item) =
            rewrite_export_alias_item(item, &plans_by_old, &plans_by_old_name, &plans_by_alias)
        {
            new_body.push(item);
        }
    }
    module.body = new_body;
}

fn rewrite_export_alias_item(
    item: ModuleItem,
    plans_by_old: &HashMap<BindingId, &ExportRenamePlan>,
    plans_by_old_name: &HashMap<Atom, &ExportRenamePlan>,
    plans_by_alias: &HashMap<Atom, &ExportRenamePlan>,
) -> Option<ModuleItem> {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(var),
            ..
        })) if is_collapsed_export_const_alias(&var, plans_by_old) => None,
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(mut named)) if named.src.is_none() => {
            let mut new_specifiers = Vec::new();
            for specifier in named.specifiers {
                match specifier {
                    ExportSpecifier::Named(mut named_specifier) => {
                        let old_name = match &named_specifier.orig {
                            ModuleExportName::Ident(ident) => ident.sym.clone(),
                            _ => {
                                new_specifiers.push(ExportSpecifier::Named(named_specifier));
                                continue;
                            }
                        };
                        // Look up by old_name directly, or by alias name
                        let Some(plan) = plans_by_old_name
                            .get(&old_name)
                            .or_else(|| plans_by_alias.get(&old_name))
                            .copied()
                        else {
                            new_specifiers.push(ExportSpecifier::Named(named_specifier));
                            continue;
                        };

                        let should_drop = matches!(
                            &named_specifier.exported,
                            Some(ModuleExportName::Ident(exported)) if exported.sym == plan.new_name
                        );
                        if should_drop {
                            continue;
                        }

                        named_specifier.orig = ModuleExportName::Ident(
                            swc_core::ecma::ast::IdentName::new(plan.new_name.clone(), DUMMY_SP)
                                .into(),
                        );

                        if named_specifier.exported.is_none() {
                            named_specifier.exported = Some(ModuleExportName::Ident(
                                swc_core::ecma::ast::IdentName::new(old_name, DUMMY_SP).into(),
                            ));
                        }

                        new_specifiers.push(ExportSpecifier::Named(named_specifier));
                    }
                    other => new_specifiers.push(other),
                }
            }

            if new_specifiers.is_empty() {
                None
            } else {
                named.specifiers = new_specifiers;
                Some(ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)))
            }
        }
        other => Some(other),
    }
}

fn is_collapsed_export_const_alias(
    var: &VarDecl,
    plans_by_old: &HashMap<BindingId, &ExportRenamePlan>,
) -> bool {
    if var.decls.len() != 1 {
        return false;
    }
    let (Pat::Ident(binding), Some(init)) = (&var.decls[0].name, &var.decls[0].init) else {
        return false;
    };
    let Expr::Ident(init_id) = init.as_ref() else {
        return false;
    };
    let Some(plan) = plans_by_old
        .get(&(init_id.sym.clone(), init_id.ctxt))
        .copied()
    else {
        return false;
    };
    binding.id.sym == plan.new_name
}

/// If the binding is a simple var alias (`var h = p`), resolve to `p`'s info.
/// Returns the resolved info and old_name to use for the rename plan.
fn resolve_to_real_binding<'a>(
    info: &'a TopLevelBindingInfo,
    name: &Atom,
    module: &Module,
    binding_infos: &'a HashMap<Atom, TopLevelBindingInfo>,
) -> (&'a TopLevelBindingInfo, Atom, Vec<Atom>) {
    let mut current_info = info;
    let mut current_name = name.clone();
    let mut seen = HashSet::new();
    let mut alias_names = Vec::new();

    loop {
        if !seen.insert(current_info.id.clone()) {
            return (current_info, current_name, alias_names);
        }

        let TopLevelBindingKind::Var { declarator_index } = &current_info.kind else {
            return (current_info, current_name, alias_names);
        };
        let Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))) =
            module.body.get(current_info.item_index)
        else {
            return (current_info, current_name, alias_names);
        };
        let Some(decl) = var.decls.get(*declarator_index) else {
            return (current_info, current_name, alias_names);
        };
        let Some(init) = &decl.init else {
            return (current_info, current_name, alias_names);
        };
        let Expr::Ident(init_id) = init.as_ref() else {
            return (current_info, current_name, alias_names);
        };
        let Some(real_info) = binding_infos.get(&init_id.sym) else {
            return (current_info, current_name, alias_names);
        };
        if real_info.exported {
            return (current_info, current_name, alias_names);
        }

        alias_names.push(current_name);
        current_info = real_info;
        current_name = init_id.sym.clone();
    }
}

/// Detect `export const ns = { get name() { return binding; }, ... }` and
/// return (getter_name, local_binding_id) pairs.
fn extract_exported_getter_namespace(item: &ModuleItem) -> Option<Vec<(Atom, BindingId)>> {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        decl: Decl::Var(var),
        ..
    })) = item
    else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }
    let init = var.decls[0].init.as_deref()?;
    let Expr::Object(obj) = init else {
        return None;
    };
    if obj.props.len() < 2 {
        return None;
    }
    let all_getters = obj
        .props
        .iter()
        .all(|p| matches!(p, PropOrSpread::Prop(prop) if matches!(prop.as_ref(), Prop::Getter(_))));
    if !all_getters {
        return None;
    }

    let mut pairs = Vec::new();
    for prop in &obj.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::Getter(getter) = prop.as_ref() else {
            return None;
        };
        let getter_name = match &getter.key {
            PropName::Ident(id) => id.sym.clone(),
            _ => return None,
        };
        let body = getter.body.as_ref()?;
        if body.stmts.len() != 1 {
            return None;
        }
        let Stmt::Return(ret) = &body.stmts[0] else {
            return None;
        };
        let Expr::Ident(id) = ret.arg.as_deref()? else {
            return None;
        };
        pairs.push((getter_name, (id.sym.clone(), id.ctxt)));
    }
    Some(pairs)
}
