use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    Decl, DefaultDecl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Expr, Ident,
    ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport,
    ObjectPatProp, Pat, PropName, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnExportRename;

type BindingId = (Atom, SyntaxContext);

impl VisitMut for UnExportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let module_names = collect_module_names(module);
        let top_level_bindings = collect_top_level_bindings(module);

        // Collect rename candidates: (old_binding, new_name)
        let mut renames: Vec<(BindingId, Atom)> = Vec::new();

        for item in &module.body {
            // Pattern 1: export const newName = oldName
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) = item
            {
                if var.decls.len() == 1 {
                    if let (Pat::Ident(id), Some(init)) = (&var.decls[0].name, &var.decls[0].init) {
                        if let Expr::Ident(init_id) = init.as_ref() {
                            let new_name = id.id.sym.clone();
                            let old_binding = (init_id.sym.clone(), init_id.ctxt);
                            // Only proceed if old_name is a top-level binding
                            // and we haven't already added it
                            if new_name != old_binding.0
                                && module_names.contains(&old_binding.0)
                                && !renames.iter().any(|(old, _)| old == &old_binding)
                            {
                                renames.push((old_binding, new_name));
                            }
                        }
                    }
                }
            }

            // Pattern 2: export { oldName as newName } (no source)
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
                specifiers,
                src: None,
                ..
            })) = item
            {
                for spec in specifiers {
                    let ExportSpecifier::Named(ExportNamedSpecifier {
                        orig,
                        exported: Some(exported),
                        ..
                    }) = spec
                    else {
                        continue;
                    };
                    let old_name = match orig {
                        ModuleExportName::Ident(i) => i.sym.clone(),
                        _ => continue,
                    };
                    let Some(old_binding) = top_level_bindings.get(&old_name).cloned() else {
                        continue;
                    };
                    let new_name = match exported {
                        ModuleExportName::Ident(i) => i.sym.clone(),
                        _ => continue,
                    };
                    // Only proceed if: names differ, old is declared, new is not yet taken
                    if old_name != new_name
                        && module_names.contains(&old_name)
                        && !module_names.contains(&new_name)
                        && !renames.iter().any(|(old, _)| old == &old_binding)
                    {
                        renames.push((old_binding, new_name));
                    }
                }
            }
        }

        if renames.is_empty() {
            return;
        }

        let renamed_old_names: HashSet<Atom> =
            renames.iter().map(|(old, _)| old.0.clone()).collect();

        // Promote each old declaration to an export declaration
        for (old_binding, _) in &renames {
            promote_to_export(module, &old_binding.0);
        }

        // Remove the export-rename statements
        module.body.retain(|item| {
            // Remove Pattern 1: export const newName = oldName
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) = item
            {
                if var.decls.len() == 1 {
                    if let (_, Some(init)) = (&var.decls[0].name, &var.decls[0].init) {
                        if let Expr::Ident(init_id) = init.as_ref() {
                            if renamed_old_names.contains(&init_id.sym) {
                                return false;
                            }
                        }
                    }
                }
            }
            // Remove Pattern 2: export { oldName as newName }
            if let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
                specifiers,
                src: None,
                ..
            })) = item
            {
                let all_removed = specifiers.iter().all(|spec| {
                    if let ExportSpecifier::Named(ExportNamedSpecifier {
                        orig,
                        exported: Some(_),
                        ..
                    }) = spec
                    {
                        if let ModuleExportName::Ident(i) = orig {
                            return renamed_old_names.contains(&i.sym);
                        }
                    }
                    false
                });
                if all_removed && !specifiers.is_empty() {
                    return false;
                }
            }
            true
        });

        // Apply renames throughout the module
        let mut renamer = Renamer { renames: &renames };
        module.visit_mut_with(&mut renamer);
    }
}

/// Wrap a top-level non-exported declaration of `name` in an ExportDecl.
fn promote_to_export(module: &mut Module, name: &Atom) {
    for item in &mut module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                let has_name = var
                    .decls
                    .iter()
                    .any(|d| matches!(&d.name, Pat::Ident(bi) if bi.id.sym == *name));
                if !has_name {
                    continue;
                }
                let taken = std::mem::replace(item, empty_module_item());
                if let ModuleItem::Stmt(Stmt::Decl(decl)) = taken {
                    *item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                        span: DUMMY_SP,
                        decl,
                    }));
                }
                return;
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) if fn_decl.ident.sym == *name => {
                let taken = std::mem::replace(item, empty_module_item());
                if let ModuleItem::Stmt(Stmt::Decl(decl)) = taken {
                    *item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                        span: DUMMY_SP,
                        decl,
                    }));
                }
                return;
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
                if class_decl.ident.sym == *name =>
            {
                let taken = std::mem::replace(item, empty_module_item());
                if let ModuleItem::Stmt(Stmt::Decl(decl)) = taken {
                    *item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                        span: DUMMY_SP,
                        decl,
                    }));
                }
                return;
            }
            _ => {}
        }
    }
}

fn empty_module_item() -> ModuleItem {
    use swc_core::ecma::ast::{EmptyStmt, Stmt};
    ModuleItem::Stmt(Stmt::Empty(EmptyStmt { span: DUMMY_SP }))
}

fn collect_module_names(module: &Module) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for spec in &import.specifiers {
                    match spec {
                        ImportSpecifier::Named(n) => {
                            names.insert(n.local.sym.clone());
                        }
                        ImportSpecifier::Default(d) => {
                            names.insert(d.local.sym.clone());
                        }
                        ImportSpecifier::Namespace(n) => {
                            names.insert(n.local.sym.clone());
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_names(decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                collect_decl_names(&e.decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(e)) => match &e.decl {
                DefaultDecl::Fn(f) => {
                    if let Some(id) = &f.ident {
                        names.insert(id.sym.clone());
                    }
                }
                DefaultDecl::Class(c) => {
                    if let Some(id) = &c.ident {
                        names.insert(id.sym.clone());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    names
}

fn collect_top_level_bindings(module: &Module) -> HashMap<Atom, BindingId> {
    let mut bindings = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => collect_decl_bindings(decl, &mut bindings),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                collect_decl_bindings(&e.decl, &mut bindings);
            }
            _ => {}
        }
    }
    bindings
}

fn collect_decl_names(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls {
                collect_pat_names(&d.name, names);
            }
        }
        Decl::Fn(f) => {
            names.insert(f.ident.sym.clone());
        }
        Decl::Class(c) => {
            names.insert(c.ident.sym.clone());
        }
        _ => {}
    }
}

fn collect_decl_bindings(decl: &Decl, bindings: &mut HashMap<Atom, BindingId>) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls {
                collect_pat_bindings(&d.name, bindings);
            }
        }
        Decl::Fn(f) => {
            bindings.insert(f.ident.sym.clone(), (f.ident.sym.clone(), f.ident.ctxt));
        }
        Decl::Class(c) => {
            bindings.insert(c.ident.sym.clone(), (c.ident.sym.clone(), c.ident.ctxt));
        }
        _ => {}
    }
}

fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(bi) => {
            names.insert(bi.id.sym.clone());
        }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_names(elem, names);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_names(&kv.value, names),
                    ObjectPatProp::Assign(a) => {
                        names.insert(a.key.id.sym.clone());
                    }
                    ObjectPatProp::Rest(r) => collect_pat_names(&r.arg, names),
                }
            }
        }
        Pat::Rest(r) => collect_pat_names(&r.arg, names),
        Pat::Assign(a) => collect_pat_names(&a.left, names),
        _ => {}
    }
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut HashMap<Atom, BindingId>) {
    match pat {
        Pat::Ident(bi) => {
            bindings.insert(bi.id.sym.clone(), (bi.id.sym.clone(), bi.id.ctxt));
        }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_bindings(&kv.value, bindings),
                    ObjectPatProp::Assign(a) => {
                        bindings
                            .insert(a.key.id.sym.clone(), (a.key.id.sym.clone(), a.key.id.ctxt));
                    }
                    ObjectPatProp::Rest(r) => collect_pat_bindings(&r.arg, bindings),
                }
            }
        }
        Pat::Rest(r) => collect_pat_bindings(&r.arg, bindings),
        Pat::Assign(a) => collect_pat_bindings(&a.left, bindings),
        _ => {}
    }
}

struct Renamer<'a> {
    renames: &'a [(BindingId, Atom)],
}

impl VisitMut for Renamer<'_> {
    fn visit_mut_ident(&mut self, id: &mut Ident) {
        for (old, new) in self.renames {
            if id.sym == old.0 && id.ctxt == old.1 {
                id.sym = new.clone();
                return;
            }
        }
    }
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
}
