use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{
    collect_module_names, rename_bindings_in_module, BindingId, BindingRename, RenameShadowIndex,
};

pub struct UnImportRename;

impl VisitMut for UnImportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut all_names = collect_module_names(module);

        let mut candidates: Vec<(BindingId, Atom)> = Vec::new();
        let mut candidate_bindings = HashSet::new();
        for item in &module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            for spec in &import.specifiers {
                let ImportSpecifier::Named(named) = spec else {
                    continue;
                };
                let local = named.local.sym.clone();
                let imported: Atom = match &named.imported {
                    Some(ModuleExportName::Ident(i)) => i.sym.clone(),
                    _ => continue, // skip Str exports and shorthand
                };
                if imported == local {
                    continue;
                }

                let local_id = (local, named.local.ctxt);
                candidate_bindings.insert(local_id.clone());
                candidates.push((local_id, imported));
            }
        }

        let shadow_index = RenameShadowIndex::for_bindings(module, &candidate_bindings);

        // Build rename list: (local_alias_binding → target based on imported name)
        let mut renames: Vec<BindingRename> = Vec::new();

        for (local_id, imported) in candidates {
            // `all_names` covers module-level collisions; `shadow_index`
            // catches inner-scope locals (e.g. a nested `let a` that would capture
            // references to the renamed import).
            let mut target = generate_unique_name(imported, &all_names);
            while shadow_index.rename_causes_shadowing(&local_id, &target) {
                all_names.insert(target.clone());
                target = generate_unique_name(target, &all_names);
            }
            all_names.insert(target.clone());
            renames.push(BindingRename {
                old: local_id,
                new: target,
            });
        }

        if !renames.is_empty() {
            rename_bindings_in_module(module, &renames);
        }

        // Clean up import { foo as foo } → import { foo }
        for item in &mut module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                continue;
            };
            for spec in &mut import.specifiers {
                let ImportSpecifier::Named(named) = spec else {
                    continue;
                };
                let is_same = match &named.imported {
                    Some(ModuleExportName::Ident(i)) => i.sym == named.local.sym,
                    Some(ModuleExportName::Str(_)) => false, // keep Str exports as-is
                    None => true,
                };
                if is_same {
                    named.imported = None;
                }
            }
        }
    }
}

fn generate_unique_name(base: Atom, existing: &HashSet<Atom>) -> Atom {
    if !existing.contains(&base) {
        return base;
    }
    let mut i = 1u32;
    loop {
        let candidate: Atom = format!("{}_{}", base, i).into();
        if !existing.contains(&candidate) {
            return candidate;
        }
        i += 1;
    }
}
