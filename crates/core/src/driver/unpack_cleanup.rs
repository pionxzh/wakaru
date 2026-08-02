use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    ExportSpecifier, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport,
};

use crate::unpacker::module_item_declared_binding_ids;

pub(crate) fn prune_stale_local_named_exports(module: &mut Module) {
    let exportable_names: std::collections::HashSet<_> = module
        .body
        .iter()
        .flat_map(|item| {
            module_item_declared_binding_ids(item)
                .into_iter()
                .map(|(sym, _)| sym)
                .chain(module_item_import_names(item))
        })
        .collect();

    module.body.retain_mut(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
            specifiers, src, ..
        })) = item
        else {
            return true;
        };

        if src.is_some() {
            return true;
        }

        specifiers.retain(|specifier| match specifier {
            ExportSpecifier::Named(named) => match &named.orig {
                ModuleExportName::Ident(local) => exportable_names.contains(&local.sym),
                ModuleExportName::Str(_) => true,
            },
            ExportSpecifier::Default(default) => exportable_names.contains(&default.exported.sym),
            ExportSpecifier::Namespace(_) => true,
        });
        !specifiers.is_empty()
    });
}

pub(crate) fn dedup_duplicate_exports(module: &mut Module) {
    let mut exported_names = HashSet::new();

    module.body.retain_mut(|item| {
        let ModuleItem::ModuleDecl(decl) = item else {
            return true;
        };

        match decl {
            ModuleDecl::ExportDecl(_) => {
                for (sym, _) in module_item_declared_binding_ids(item) {
                    exported_names.insert(sym.to_string());
                }
                true
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                exported_names.insert("default".to_string())
            }
            ModuleDecl::ExportNamed(named) => {
                if named.src.is_some() {
                    return true;
                }

                named.specifiers.retain(|specifier| {
                    let Some(exported_name) = export_specifier_name(specifier) else {
                        return true;
                    };
                    exported_names.insert(exported_name)
                });
                !named.specifiers.is_empty()
            }
            _ => true,
        }
    });
}

fn export_specifier_name(specifier: &ExportSpecifier) -> Option<String> {
    match specifier {
        ExportSpecifier::Named(named) => Some(module_export_name_string(
            named.exported.as_ref().unwrap_or(&named.orig),
        )),
        ExportSpecifier::Default(_) => Some("default".to_string()),
        ExportSpecifier::Namespace(namespace) => Some(module_export_name_string(&namespace.name)),
    }
}

fn module_export_name_string(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(str_lit) => str_lit.value.as_str().unwrap_or("").to_string(),
    }
}

fn module_item_import_names(item: &ModuleItem) -> Vec<Atom> {
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
        return vec![];
    };
    import
        .specifiers
        .iter()
        .map(|specifier| match specifier {
            ImportSpecifier::Named(named) => named.local.sym.clone(),
            ImportSpecifier::Default(default) => default.local.sym.clone(),
            ImportSpecifier::Namespace(namespace) => namespace.local.sym.clone(),
        })
        .collect()
}
