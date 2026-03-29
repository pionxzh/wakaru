use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, Decl, DefaultDecl, Function, Ident, ImportSpecifier, MemberProp, Module, ModuleDecl,
    ModuleItem, ObjectPatProp, Pat, PropName, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub type BindingId = (Atom, SyntaxContext);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingRename {
    pub old: BindingId,
    pub new: Atom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopLevelBindingKind {
    Var { declarator_index: usize },
    Fn,
    Class,
}

#[derive(Clone, Debug)]
pub struct TopLevelBindingInfo {
    pub id: BindingId,
    pub item_index: usize,
    pub exported: bool,
    pub kind: TopLevelBindingKind,
}

pub fn collect_module_names(module: &Module) -> HashSet<Atom> {
    let mut names = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for spec in &import.specifiers {
                    match spec {
                        ImportSpecifier::Named(named) => {
                            names.insert(named.local.sym.clone());
                        }
                        ImportSpecifier::Default(default) => {
                            names.insert(default.local.sym.clone());
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            names.insert(namespace.local.sym.clone());
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_names(decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                collect_decl_names(&export_decl.decl, &mut names);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(default_decl)) => {
                match &default_decl.decl {
                    DefaultDecl::Fn(fn_expr) => {
                        if let Some(ident) = &fn_expr.ident {
                            names.insert(ident.sym.clone());
                        }
                    }
                    DefaultDecl::Class(class_expr) => {
                        if let Some(ident) = &class_expr.ident {
                            names.insert(ident.sym.clone());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    names
}

pub fn collect_top_level_binding_infos(module: &Module) -> HashMap<Atom, TopLevelBindingInfo> {
    let mut infos = HashMap::new();

    for (item_index, item) in module.body.iter().enumerate() {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_decl_binding_infos(decl, item_index, false, &mut infos);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export_decl)) => {
                collect_decl_binding_infos(&export_decl.decl, item_index, true, &mut infos);
            }
            _ => {}
        }
    }

    infos
}

pub fn rename_causes_shadowing(module: &Module, old: &BindingId, new_name: &Atom) -> bool {
    struct Checker<'a> {
        old: &'a BindingId,
        new_name: &'a Atom,
        scope_stack: Vec<(bool, bool)>,
        found: bool,
    }

    impl Checker<'_> {
        fn pat_binds_new(&self, pat: &Pat) -> bool {
            match pat {
                Pat::Ident(id) => &id.id.sym == self.new_name,
                Pat::Array(arr) => arr.elems.iter().flatten().any(|p| self.pat_binds_new(p)),
                Pat::Object(obj) => obj.props.iter().any(|p| match p {
                    ObjectPatProp::KeyValue(kv) => self.pat_binds_new(&kv.value),
                    ObjectPatProp::Assign(assign) => &assign.key.id.sym == self.new_name,
                    ObjectPatProp::Rest(rest) => self.pat_binds_new(&rest.arg),
                }),
                Pat::Assign(assign) => self.pat_binds_new(&assign.left),
                Pat::Rest(rest) => self.pat_binds_new(&rest.arg),
                _ => false,
            }
        }

        fn on_exit_scope(&mut self) {
            if let Some((declares, refs_old)) = self.scope_stack.pop() {
                if declares && refs_old {
                    self.found = true;
                }
            }
        }
    }

    impl Visit for Checker<'_> {
        fn visit_function(&mut self, function: &Function) {
            let params_declare = function.params.iter().any(|param| self.pat_binds_new(&param.pat));
            self.scope_stack.push((params_declare, false));
            function.visit_children_with(self);
            self.on_exit_scope();
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            let params_declare = arrow.params.iter().any(|param| self.pat_binds_new(param));
            self.scope_stack.push((params_declare, false));
            arrow.visit_children_with(self);
            self.on_exit_scope();
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            if !self.scope_stack.is_empty() && self.pat_binds_new(&declarator.name) {
                if let Some(top) = self.scope_stack.last_mut() {
                    top.0 = true;
                }
            }
            declarator.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.old.0 && ident.ctxt == self.old.1 {
                for scope in &mut self.scope_stack {
                    scope.1 = true;
                }
            }
        }

        fn visit_prop_name(&mut self, _: &PropName) {}

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(computed) = prop {
                computed.visit_children_with(self);
            }
        }
    }

    let mut checker = Checker {
        old,
        new_name,
        scope_stack: Vec::new(),
        found: false,
    };
    module.visit_with(&mut checker);
    checker.found
}

pub fn rename_bindings_in_module(module: &mut Module, renames: &[BindingRename]) {
    if renames.is_empty() {
        return;
    }
    let mut renamer = BindingRenamer { renames };
    module.visit_mut_with(&mut renamer);
}

pub fn rename_bindings<T>(node: &mut T, renames: &[BindingRename])
where
    for<'a> T: VisitMutWith<BindingRenamer<'a>>,
{
    if renames.is_empty() {
        return;
    }
    let mut renamer = BindingRenamer { renames };
    node.visit_mut_with(&mut renamer);
}

fn collect_decl_names(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => {
            for declarator in &var.decls {
                collect_pat_names(&declarator.name, names);
            }
        }
        Decl::Fn(function) => {
            names.insert(function.ident.sym.clone());
        }
        Decl::Class(class) => {
            names.insert(class.ident.sym.clone());
        }
        _ => {}
    }
}

fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(binding) => {
            names.insert(binding.id.sym.clone());
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_names(elem, names);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_names(&kv.value, names),
                    ObjectPatProp::Assign(assign) => {
                        names.insert(assign.key.id.sym.clone());
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_names(&rest.arg, names),
                }
            }
        }
        Pat::Rest(rest) => collect_pat_names(&rest.arg, names),
        Pat::Assign(assign) => collect_pat_names(&assign.left, names),
        _ => {}
    }
}

fn collect_decl_binding_infos(
    decl: &Decl,
    item_index: usize,
    exported: bool,
    infos: &mut HashMap<Atom, TopLevelBindingInfo>,
) {
    match decl {
        Decl::Var(var) => {
            for (declarator_index, declarator) in var.decls.iter().enumerate() {
                let Pat::Ident(binding) = &declarator.name else {
                    continue;
                };
                infos.insert(
                    binding.id.sym.clone(),
                    TopLevelBindingInfo {
                        id: (binding.id.sym.clone(), binding.id.ctxt),
                        item_index,
                        exported,
                        kind: TopLevelBindingKind::Var { declarator_index },
                    },
                );
            }
        }
        Decl::Fn(function) => {
            infos.insert(
                function.ident.sym.clone(),
                TopLevelBindingInfo {
                    id: (function.ident.sym.clone(), function.ident.ctxt),
                    item_index,
                    exported,
                    kind: TopLevelBindingKind::Fn,
                },
            );
        }
        Decl::Class(class) => {
            infos.insert(
                class.ident.sym.clone(),
                TopLevelBindingInfo {
                    id: (class.ident.sym.clone(), class.ident.ctxt),
                    item_index,
                    exported,
                    kind: TopLevelBindingKind::Class,
                },
            );
        }
        _ => {}
    }
}

pub(super) struct BindingRenamer<'a> {
    renames: &'a [BindingRename],
}

impl VisitMut for BindingRenamer<'_> {
    fn visit_mut_ident(&mut self, ident: &mut Ident) {
        for rename in self.renames {
            if ident.sym == rename.old.0 && ident.ctxt == rename.old.1 {
                ident.sym = rename.new.clone();
                return;
            }
        }
    }

    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_mut_with(self);
        }
    }
}
