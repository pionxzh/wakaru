use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    Decl, DefaultDecl, Ident, ImportSpecifier, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, ObjectPatProp, Pat, PropName, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnImportRename;

type BindingId = (Atom, SyntaxContext);

impl VisitMut for UnImportRename {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let mut all_names = collect_module_names(module);

        // Build rename list: (local_alias_binding → target based on imported name)
        let mut renames: Vec<(BindingId, Atom)> = Vec::new();

        for item in &module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else { continue };
            for spec in &import.specifiers {
                let ImportSpecifier::Named(named) = spec else { continue };
                let local = named.local.sym.clone();
                let imported: Atom = match &named.imported {
                    Some(ModuleExportName::Ident(i)) => i.sym.clone(),
                    _ => continue, // skip Str exports and shorthand
                };
                if imported == local { continue; }

                let target = generate_unique_name(imported, &all_names);
                all_names.insert(target.clone());
                renames.push(((local, named.local.ctxt), target));
            }
        }

        if !renames.is_empty() {
            let mut renamer = Renamer { renames: &renames };
            module.visit_mut_with(&mut renamer);
        }

        // Clean up import { foo as foo } → import { foo }
        for item in &mut module.body {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else { continue };
            for spec in &mut import.specifiers {
                let ImportSpecifier::Named(named) = spec else { continue };
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

fn collect_module_names(module: &Module) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for spec in &import.specifiers {
                    match spec {
                        ImportSpecifier::Named(n) => { names.insert(n.local.sym.clone()); }
                        ImportSpecifier::Default(d) => { names.insert(d.local.sym.clone()); }
                        ImportSpecifier::Namespace(n) => { names.insert(n.local.sym.clone()); }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_names(decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(e)) => {
                collect_decl_names(&e.decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(e)) => {
                match &e.decl {
                    DefaultDecl::Fn(f) => {
                        if let Some(id) = &f.ident { names.insert(id.sym.clone()); }
                    }
                    DefaultDecl::Class(c) => {
                        if let Some(id) = &c.ident { names.insert(id.sym.clone()); }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_decl_names(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls { collect_pat_names(&d.name, names); }
        }
        Decl::Fn(f) => { names.insert(f.ident.sym.clone()); }
        Decl::Class(c) => { names.insert(c.ident.sym.clone()); }
        _ => {}
    }
}

fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(bi) => { names.insert(bi.id.sym.clone()); }
        Pat::Array(arr) => {
            for elem in arr.elems.iter().flatten() { collect_pat_names(elem, names); }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_names(&kv.value, names),
                    ObjectPatProp::Assign(a) => { names.insert(a.key.id.sym.clone()); }
                    ObjectPatProp::Rest(r) => collect_pat_names(&r.arg, names),
                }
            }
        }
        Pat::Rest(r) => collect_pat_names(&r.arg, names),
        Pat::Assign(a) => collect_pat_names(&a.left, names),
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
    // Skip object literal keys (dot notation)
    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}
    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(c) = prop {
            c.visit_mut_with(self);
        }
    }
}
