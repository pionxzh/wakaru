use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    Module, ModuleDecl, ModuleExportName, ModuleItem, ImportSpecifier,
};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{collect_module_names, rename_bindings_in_module, BindingRename};

pub struct UnImportRename;

impl VisitMut for UnImportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut all_names = collect_module_names(module);

        // Build rename list: (local_alias_binding → target based on imported name)
        let mut renames: Vec<BindingRename> = Vec::new();

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

                let target = generate_unique_name(imported, &all_names);
                all_names.insert(target.clone());
                renames.push(BindingRename {
                    old: (local, named.local.ctxt),
                    new: target,
                });
            }
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
