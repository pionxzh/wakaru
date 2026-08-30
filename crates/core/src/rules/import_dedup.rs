use std::collections::{HashMap, HashSet};

use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ImportDecl, ImportPhase, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{rename_bindings_in_module, BindingId, BindingRename, RenameShadowIndex};

/// Deduplicates and merges ESM imports.
///
/// 1. **Dedup**: when the same specifier is imported multiple times from the same
///    module request (common in scope-hoisted bundles), keeps the first binding as
///    canonical, rewrites all uses of duplicates, and removes redundant specifiers.
///
/// 2. **Merge**: consolidates separate import statements with the same module request
///    into a single statement (e.g. three `import ... from "path"` → one).
pub struct ImportDedup;

impl VisitMut for ImportDedup {
    fn visit_mut_module(&mut self, module: &mut Module) {
        dedup_imports(module);
        merge_imports(module);
    }
}

/// Identifies the "imported-from-module" name of a specifier.
/// - `import { foo }` → `Named("foo")`  (shorthand: imported == local)
/// - `import { foo as bar }` → `Named("foo")`
/// - `import defaultExport from "m"` → `Default`
/// - `import * as ns from "m"` → `Namespace`
/// - `import { "string-key" as bar }` → `None` (unsupported, skipped)
#[derive(PartialEq, Eq, Hash, Clone)]
enum ImportKey {
    Named(Atom),
    Default,
    Namespace,
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct ImportRequestKey {
    src: Wtf8Atom,
    with: Option<Vec<(Atom, Wtf8Atom)>>,
    phase: ImportPhase,
}

fn import_request_key(import: &ImportDecl) -> Option<ImportRequestKey> {
    let with = match &import.with {
        Some(attributes) => {
            let attributes = attributes.as_import_with()?;
            let mut seen_keys = HashSet::new();
            let mut normalized = Vec::with_capacity(attributes.values.len());
            for item in attributes.values {
                if !seen_keys.insert(item.key.sym.clone()) {
                    return None;
                }
                normalized.push((item.key.sym, item.value.value));
            }
            Some(normalized)
        }
        None => None,
    };

    Some(ImportRequestKey {
        src: import.src.value.clone(),
        with,
        phase: import.phase,
    })
}

fn spec_key_and_local(spec: &ImportSpecifier) -> Option<(ImportKey, Atom, SyntaxContext)> {
    match spec {
        ImportSpecifier::Named(named) => {
            let imported_sym = match &named.imported {
                Some(ModuleExportName::Ident(i)) => i.sym.clone(),
                Some(ModuleExportName::Str(_)) => return None, // string re-exports – skip
                None => named.local.sym.clone(), // shorthand: imported name == local name
            };
            Some((
                ImportKey::Named(imported_sym),
                named.local.sym.clone(),
                named.local.ctxt,
            ))
        }
        ImportSpecifier::Default(d) => {
            Some((ImportKey::Default, d.local.sym.clone(), d.local.ctxt))
        }
        ImportSpecifier::Namespace(n) => {
            Some((ImportKey::Namespace, n.local.sym.clone(), n.local.ctxt))
        }
    }
}

fn dedup_imports(module: &mut Module) {
    let candidate_bindings: HashSet<BindingId> = module
        .body
        .iter()
        .filter_map(|item| {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                return None;
            };
            Some(import.specifiers.iter().filter_map(|specifier| {
                spec_key_and_local(specifier).map(|(_, sym, ctxt)| (sym, ctxt))
            }))
        })
        .flatten()
        .collect();
    let shadow_index = RenameShadowIndex::for_bindings(module, &candidate_bindings);

    // (module request, ImportKey) → canonical local (sym, ctxt)
    let mut canonical: HashMap<(ImportRequestKey, ImportKey), (Atom, SyntaxContext)> =
        HashMap::new();
    let mut renames: Vec<BindingRename> = Vec::new();
    // Set of (module item index, specifier index) for duplicate specifiers.
    let mut to_remove: HashSet<(usize, usize)> = HashSet::new();

    for (item_index, item) in module.body.iter().enumerate() {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let Some(request) = import_request_key(import) else {
            continue;
        };

        for (specifier_index, spec) in import.specifiers.iter().enumerate() {
            let Some((key, local_sym, local_ctxt)) = spec_key_and_local(spec) else {
                continue;
            };

            let map_key = (request.clone(), key);
            if let Some(entry) = canonical.get(&map_key) {
                let local_id = (local_sym.clone(), local_ctxt);
                if *entry != local_id && shadow_index.rename_causes_shadowing(&local_id, &entry.0) {
                    // Both imports are semantically equivalent, but their
                    // local names are not interchangeable at every use site.
                    // Keep this alias when the canonical name would resolve
                    // to a nested local after printing.
                    continue;
                }
                // This specifier is a duplicate. Remove the occurrence even
                // when it has the same binding id as the canonical specifier;
                // resolver can assign identical top-level contexts to exact
                // duplicate imports.
                to_remove.insert((item_index, specifier_index));
                if *entry != local_id {
                    renames.push(BindingRename {
                        old: local_id,
                        new: entry.0.clone(),
                    });
                }
            } else {
                canonical.insert(map_key, (local_sym, local_ctxt));
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    // Remove duplicate specifiers first (before renaming, while indexes still match).
    for (item_index, item) in module.body.iter_mut().enumerate() {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let mut specifier_index = 0;
        import.specifiers.retain(|_| {
            let keep = !to_remove.contains(&(item_index, specifier_index));
            specifier_index += 1;
            keep
        });
    }

    // Remove now-empty import declarations
    module.body.retain(|item| {
        !matches!(item,
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
            if import.specifiers.is_empty()
        )
    });

    if !renames.is_empty() {
        // Rewrite all uses of duplicate bindings to the canonical local name.
        rename_bindings_in_module(module, &renames);
    }
}

/// Merges separate import statements with the same module request into one statement.
///
/// ```js
/// import { join } from "path";
/// import { resolve } from "path";
/// // → import { join, resolve } from "path";
/// ```
///
/// Namespace imports (`import * as ns`) are kept as separate statements since they
/// cannot be combined with named/default specifiers.
fn merge_imports(module: &mut Module) {
    let mut first_import: HashMap<ImportRequestKey, usize> = HashMap::new();
    let mut merged_indices: HashSet<usize> = HashSet::new();

    let import_indices: Vec<usize> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))).then_some(i)
        })
        .collect();

    for &idx in &import_indices {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &module.body[idx] else {
            continue;
        };

        let has_namespace = import
            .specifiers
            .iter()
            .any(|s| matches!(s, ImportSpecifier::Namespace(_)));
        if has_namespace || import.type_only {
            continue;
        }

        let Some(request) = import_request_key(import) else {
            continue;
        };

        let has_default = import
            .specifiers
            .iter()
            .any(|s| matches!(s, ImportSpecifier::Default(_)));

        if let Some(&first_idx) = first_import.get(&request) {
            let first_has_default = {
                let ModuleItem::ModuleDecl(ModuleDecl::Import(first)) = &module.body[first_idx]
                else {
                    continue;
                };
                first
                    .specifiers
                    .iter()
                    .any(|s| matches!(s, ImportSpecifier::Default(_)))
            };

            // Can't merge two default specifiers into one import statement.
            if has_default && first_has_default {
                continue;
            }

            let specs: Vec<ImportSpecifier> = {
                let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = &module.body[idx] else {
                    continue;
                };
                import.specifiers.clone()
            };

            let ModuleItem::ModuleDecl(ModuleDecl::Import(first)) = &mut module.body[first_idx]
            else {
                continue;
            };
            first.specifiers.extend(specs);
            merged_indices.insert(idx);
        } else {
            first_import.insert(request, idx);
        }
    }

    if merged_indices.is_empty() {
        return;
    }

    let mut i = 0;
    module.body.retain(|_| {
        let keep = !merged_indices.contains(&i);
        i += 1;
        keep
    });
}
