use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{rename_bindings_in_module, BindingId, BindingRename};

// Module source paths are always valid UTF-8 in practice.
fn src_to_key(src: &swc_core::ecma::ast::Str) -> String {
    src.value.as_str().unwrap_or("").to_string()
}

/// Merges duplicate ESM imports from the same module with the same imported specifier.
///
/// Bundlers (esbuild, rollup) inline multiple source files into a single output, which
/// produces repeated imports of the same specifier:
///
/// ```js
/// import { createHash } from "crypto";
/// import { createHash as createHash_1 } from "crypto";
/// import { createHash as createHash_2 } from "crypto";
/// ```
///
/// This rule picks the first local binding as canonical, rewrites all uses of the
/// duplicates to the canonical name, and removes the redundant import declarations.
pub struct ImportDedup;

impl VisitMut for ImportDedup {
    fn visit_mut_module(&mut self, module: &mut Module) {
        dedup_imports(module);
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
    // (source_module, ImportKey) → canonical local (sym, ctxt)
    let mut canonical: HashMap<(String, ImportKey), (Atom, SyntaxContext)> = HashMap::new();
    let mut renames: Vec<BindingRename> = Vec::new();
    // Set of (sym, ctxt) for specifiers that should be removed
    let mut to_remove: HashSet<BindingId> = HashSet::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        let src = src_to_key(&import.src);

        for spec in &import.specifiers {
            let Some((key, local_sym, local_ctxt)) = spec_key_and_local(spec) else {
                continue;
            };

            let map_key = (src.clone(), key);
            let entry = canonical
                .entry(map_key)
                .or_insert_with(|| (local_sym.clone(), local_ctxt));

            if *entry != (local_sym.clone(), local_ctxt) {
                // This specifier is a duplicate — schedule it for removal and rename
                to_remove.insert((local_sym.clone(), local_ctxt));
                renames.push(BindingRename {
                    old: (local_sym, local_ctxt),
                    new: entry.0.clone(),
                });
            }
        }
    }

    if renames.is_empty() {
        return;
    }

    // Remove duplicate specifiers first (before renaming, while (sym, ctxt) still match)
    for item in &mut module.body {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        import.specifiers.retain(|spec| {
            let Some((_, local_sym, local_ctxt)) = spec_key_and_local(spec) else {
                return true; // keep unsupported specifiers
            };
            !to_remove.contains(&(local_sym, local_ctxt))
        });
    }

    // Remove now-empty import declarations
    module.body.retain(|item| {
        !matches!(item,
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
            if import.specifiers.is_empty()
        )
    });

    // Rewrite all uses of duplicate bindings to the canonical local name
    rename_bindings_in_module(module, &renames);
}
