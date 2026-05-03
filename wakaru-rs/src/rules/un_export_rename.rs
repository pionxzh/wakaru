use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Decl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Expr, Module, ModuleDecl,
    ModuleExportName, ModuleItem, NamedExport, Pat, Stmt, VarDecl,
};
use swc_core::ecma::visit::VisitMut;

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
    /// If the export used an alias (var h = p), this is `h` so we can
    /// match the export specifier's orig for cleanup.
    alias_name: Option<Atom>,
}

impl VisitMut for UnExportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let module_names = collect_module_names(module);
        let binding_infos = collect_top_level_binding_infos(module);
        let plans = collect_export_rename_plans(module, &module_names, &binding_infos);

        if plans.is_empty() {
            return;
        }

        promote_renamed_bindings(module, &plans, &binding_infos);
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
            if let Some(alias) = &plan.alias_name {
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
    _module_names: &std::collections::HashSet<Atom>,
    binding_infos: &HashMap<Atom, TopLevelBindingInfo>,
) -> Vec<ExportRenamePlan> {
    let mut plans = Vec::new();

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
                            && !plans
                                .iter()
                                .any(|plan: &ExportRenamePlan| plan.old == info.id)
                            && !rename_causes_shadowing(module, &info.id, &new_name)
                        {
                            plans.push(ExportRenamePlan {
                                old: info.id.clone(),
                                old_name: info.id.0.clone(),
                                new_name,
                                alias_name: None,
                            });
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
                let (info, old_name) =
                    resolve_to_real_binding(orig_info, &orig_name, module, binding_infos);
                let new_name = match exported {
                    ModuleExportName::Ident(ident) => ident.sym.clone(),
                    _ => continue,
                };
                // Skip if the export name is shorter than the local name —
                // that would replace a more meaningful name with a less meaningful one.
                if old_name == new_name
                    || new_name.len() < old_name.len()
                    || binding_infos.contains_key(&new_name)
                    || plans
                        .iter()
                        .any(|plan: &ExportRenamePlan| plan.old == info.id)
                    || rename_causes_shadowing(module, &info.id, &new_name)
                {
                    continue;
                }
                let alias_name = if old_name != orig_name {
                    Some(orig_name.clone())
                } else {
                    None
                };
                plans.push(ExportRenamePlan {
                    old: info.id.clone(),
                    old_name,
                    new_name,
                    alias_name,
                });
            }
        }
    }

    plans
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
        new_body.extend(rewrite_promoted_item(item, plan, info));
    }
    module.body = new_body;
}

fn rewrite_promoted_item(
    item: ModuleItem,
    plan: &ExportRenamePlan,
    info: &TopLevelBindingInfo,
) -> Vec<ModuleItem> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => rewrite_var_decl(*var, false, plan, info)
            .unwrap_or_else(|var| vec![ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(var))))]),
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(var),
            ..
        })) => rewrite_var_decl(*var, true, plan, info)
            .unwrap_or_else(|var| vec![wrap_var_decl(var, true)]),
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(mut function))) => {
            function.ident.sym = plan.new_name.clone();
            vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: DUMMY_SP,
                decl: Decl::Fn(function),
            }))]
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(mut class))) => {
            class.ident.sym = plan.new_name.clone();
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
    plan: &ExportRenamePlan,
    info: &TopLevelBindingInfo,
) -> Result<Vec<ModuleItem>, VarDecl> {
    let TopLevelBindingKind::Var { declarator_index } = info.kind else {
        return Err(var);
    };
    if declarator_index >= var.decls.len() {
        return Err(var);
    }

    let mut target = var.decls[declarator_index].clone();
    let Pat::Ident(binding) = &mut target.name else {
        return Err(var);
    };
    binding.id.sym = plan.new_name.clone();

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
        .filter_map(|plan| plan.alias_name.as_ref().map(|a| (a.clone(), plan)))
        .collect();

    // Collect alias var names to remove (var h = p where h was the alias)
    let alias_var_names: std::collections::HashSet<Atom> = plans
        .iter()
        .filter_map(|plan| plan.alias_name.clone())
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
) -> (&'a TopLevelBindingInfo, Atom) {
    let TopLevelBindingKind::Var { declarator_index } = &info.kind else {
        return (info, name.clone());
    };
    let Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))) = module.body.get(info.item_index)
    else {
        return (info, name.clone());
    };
    let Some(decl) = var.decls.get(*declarator_index) else {
        return (info, name.clone());
    };
    let Some(init) = &decl.init else {
        return (info, name.clone());
    };
    let Expr::Ident(init_id) = init.as_ref() else {
        return (info, name.clone());
    };
    if let Some(real_info) = binding_infos.get(&init_id.sym) {
        if !real_info.exported {
            return (real_info, init_id.sym.clone());
        }
    }
    (info, name.clone())
}
