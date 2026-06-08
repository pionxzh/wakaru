use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    Callee, Decl, ExportSpecifier, Expr, ImportSpecifier, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, NamedExport, Pat, Stmt, VarDeclarator,
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

pub(crate) fn hoist_late_runtime_helpers(module: &mut Module) {
    let Some(import_end) = module
        .body
        .iter()
        .position(|item| !matches!(item, ModuleItem::ModuleDecl(ModuleDecl::Import(_))))
    else {
        return;
    };

    let mut imports_and_early_items = Vec::new();
    let mut hoisted = Vec::new();
    let mut rest = Vec::new();
    let mut seen_side_effect = false;
    let mut hoisted_namespace_names = HashSet::new();

    for (index, item) in module.body.drain(..).enumerate() {
        if index < import_end {
            imports_and_early_items.push(item);
            continue;
        }

        let hoistable_late_item = if seen_side_effect {
            if is_hoistable_runtime_helper_item(&item) {
                true
            } else if let Some(names) = exported_empty_object_var_names(&item) {
                hoisted_namespace_names.extend(names);
                true
            } else {
                is_define_property_for_names(&item, &hoisted_namespace_names)
            }
        } else {
            false
        };

        if hoistable_late_item {
            hoisted.push(item);
            continue;
        }

        if !is_hoistable_runtime_helper_item(&item) {
            seen_side_effect = true;
        }
        rest.push(item);
    }

    if hoisted.is_empty() {
        module.body = imports_and_early_items;
        module.body.extend(rest);
        return;
    }

    imports_and_early_items.extend(hoisted);
    imports_and_early_items.extend(rest);
    module.body = imports_and_early_items;
}

fn is_hoistable_runtime_helper_item(item: &ModuleItem) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_))) => true,
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            var.decls.iter().all(is_hoistable_runtime_helper_var)
        }
        _ => false,
    }
}

fn is_hoistable_runtime_helper_var(decl: &VarDeclarator) -> bool {
    let Some(init) = decl.init.as_deref() else {
        return true;
    };

    matches!(init, Expr::Fn(_) | Expr::Arrow(_))
        || is_object_destructure_from_object(&decl.name, init)
}

fn is_object_destructure_from_object(name: &Pat, init: &Expr) -> bool {
    matches!(name, Pat::Object(_)) && matches!(init, Expr::Ident(ident) if ident.sym == *"Object")
}

fn exported_empty_object_var_names(item: &ModuleItem) -> Option<Vec<String>> {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) = item else {
        return None;
    };
    let Decl::Var(var) = &export.decl else {
        return None;
    };

    let mut names = Vec::new();
    for decl in &var.decls {
        let Pat::Ident(ident) = &decl.name else {
            return None;
        };
        let init = decl.init.as_deref()?;
        let Expr::Object(object) = init else {
            return None;
        };
        if !object.props.is_empty() {
            return None;
        }
        names.push(ident.id.sym.to_string());
    }

    Some(names)
}

fn is_define_property_for_names(item: &ModuleItem, names: &HashSet<String>) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(stmt)) = item else {
        return false;
    };
    let Expr::Call(call) = stmt.expr.as_ref() else {
        return false;
    };
    if !is_object_define_property_callee(&call.callee) {
        return false;
    }
    let Some(first_arg) = call.args.first() else {
        return false;
    };
    matches!(first_arg.expr.as_ref(), Expr::Ident(ident) if names.contains(ident.sym.as_ref()))
}

fn is_object_define_property_callee(callee: &Callee) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    let Expr::Member(member) = expr.as_ref() else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(obj) if obj.sym == *"Object")
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == *"defineProperty")
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

pub(crate) fn module_export_name_string(name: &ModuleExportName) -> String {
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
