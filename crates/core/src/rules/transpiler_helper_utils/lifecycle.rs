//! Helper declaration lifecycle: reference tracking across the module, the
//! top-level callable dependency graph, and removal of helper declarations once
//! all their call sites have been rewritten.

use std::collections::{HashMap, HashSet};

use swc_core::ecma::ast::{Decl, Expr, Module, ModuleItem, Pat, Stmt};

use super::*;

/// Check which helper bindings still have references in the module body,
/// excluding the declaration binding itself (VarDeclarator name / FnDecl ident).
/// Catches both remaining calls and aliasing (`var f = helper`).
pub(crate) fn helpers_with_remaining_refs(
    module: &Module,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
) -> HashSet<BindingKey> {
    let helper_keys: HashSet<_> = helpers.keys().cloned().collect();
    remaining_refs_outside_declarations(module, &helper_keys, &helper_keys)
}
pub(crate) fn remove_helpers_without_remaining_refs(
    module: &mut Module,
    helpers: HashMap<BindingKey, TranspilerHelperKind>,
) {
    let remaining = helpers_with_remaining_refs(module, &helpers);
    let safe_to_remove: HashMap<BindingKey, TranspilerHelperKind> = helpers
        .into_iter()
        .filter(|(key, _)| !remaining.contains(key))
        .collect();
    if !safe_to_remove.is_empty() {
        remove_helper_declarations(&mut module.body, &safe_to_remove);
    }
}
pub(super) fn helper_dependencies_from_ref_graph(
    ref_graph: &HashMap<BindingKey, HashSet<BindingKey>>,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    let mut dependencies = HashSet::new();
    let mut stack: Vec<_> = helpers.keys().cloned().collect();

    while let Some(key) = stack.pop() {
        let Some(refs) = ref_graph.get(&key) else {
            continue;
        };
        for dep in refs {
            if helpers.contains_key(dep) || !dependencies.insert(dep.clone()) {
                continue;
            }
            stack.push(dep.clone());
        }
    }

    dependencies
        .into_iter()
        .map(|key| (key, TranspilerHelperKind::HelperDependency))
        .collect()
}
pub(super) fn collect_top_level_callable_ref_graph(
    module: &Module,
) -> HashMap<BindingKey, HashSet<BindingKey>> {
    let mut candidates = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                candidates.insert((fn_decl.ident.sym.clone(), fn_decl.ident.ctxt));
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if !matches!(
                        decl.init.as_deref(),
                        Some(Expr::Fn(_)) | Some(Expr::Arrow(_))
                    ) {
                        continue;
                    }
                    if let Pat::Ident(binding) = &decl.name {
                        candidates.insert((binding.id.sym.clone(), binding.id.ctxt));
                    }
                }
            }
            _ => {}
        }
    }

    let mut refs = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = (fn_decl.ident.sym.clone(), fn_decl.ident.ctxt);
                if candidates.contains(&key) {
                    refs.insert(key, collect_refs(&fn_decl.function, &candidates));
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Pat::Ident(binding) = &decl.name else {
                        continue;
                    };
                    let key = (binding.id.sym.clone(), binding.id.ctxt);
                    if !candidates.contains(&key) {
                        continue;
                    }
                    if let Some(init) = &decl.init {
                        refs.insert(key, collect_refs(init, &candidates));
                    }
                }
            }
            _ => {}
        }
    }
    refs
}
/// Remove helper declarations from the module body.
pub(crate) fn remove_helper_declarations(
    body: &mut Vec<ModuleItem>,
    helpers: &HashMap<BindingKey, TranspilerHelperKind>,
) {
    let helper_keys: HashSet<_> = helpers.keys().cloned().collect();
    remove_fn_decls_from_body_by_binding(body, &helper_keys);
    remove_var_declarators_by_binding(body, &helper_keys);
    remove_import_specifiers_by_binding(body, &helper_keys);
}
