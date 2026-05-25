use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    Expr, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

use super::rename_utils::{
    collect_module_names, rename_bindings_in_module, BindingId, BindingRename, RenameShadowIndex,
};

pub struct UnImportRename {
    unresolved_mark: Mark,
}

impl UnImportRename {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnImportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut all_names = collect_module_names(module);
        all_names.extend(collect_unresolved_reference_names(
            module,
            self.unresolved_mark,
        ));

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
                if is_reserved_binding_name(imported.as_ref()) {
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

fn collect_unresolved_reference_names(module: &Module, unresolved_mark: Mark) -> HashSet<Atom> {
    let mut collector = UnresolvedReferenceNameCollector {
        unresolved_mark,
        names: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.names
}

struct UnresolvedReferenceNameCollector {
    unresolved_mark: Mark,
    names: HashSet<Atom>,
}

impl Visit for UnresolvedReferenceNameCollector {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(ident) = expr {
            if ident.ctxt.outer() == self.unresolved_mark {
                self.names.insert(ident.sym.clone());
            }
        }
        expr.visit_children_with(self);
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

fn is_reserved_binding_name(name: &str) -> bool {
    matches!(
        name,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "arguments"
            | "eval"
    )
}
