use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmt, CatchClause, Class, Decl, DefaultDecl, Expr, Function, Ident, ImportDecl,
    ImportNamedSpecifier, ImportSpecifier, KeyValueProp, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, ObjectPatProp, Pat, Prop, PropName, Stmt, VarDeclarator,
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
            let params_declare = function
                .params
                .iter()
                .any(|param| self.pat_binds_new(&param.pat));
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

/// Returns true when replacing references to `old` with an identifier named
/// `replacement_name` would resolve to a shadowing binding at any use site.
///
/// This is a targeted version of SWC's rename safety check: instead of running
/// a whole-module mangle pass, callers can ask whether a raw `Expr::Ident`
/// substitution would be captured by a nested function, block, or catch scope.
pub fn binding_replacement_would_be_shadowed(
    module: &Module,
    old: &BindingId,
    replacement_name: &Atom,
) -> bool {
    struct Checker<'a> {
        old: &'a BindingId,
        replacement_name: &'a Atom,
        scope_stack: Vec<bool>,
        found: bool,
    }

    impl Checker<'_> {
        fn in_shadowing_scope(&self) -> bool {
            self.scope_stack.iter().any(|declares| *declares)
        }

        fn pat_binds_replacement(&self, pat: &Pat) -> bool {
            pat_binds_name(pat, self.replacement_name)
        }

        fn block_binds_replacement(&self, block: &BlockStmt) -> bool {
            block_binds_name(block, self.replacement_name)
        }

        fn is_old_ident(&self, ident: &Ident) -> bool {
            ident.sym == self.old.0 && ident.ctxt == self.old.1
        }

        fn mark_current_scope_binding(&mut self, pat: &Pat) {
            if !self.scope_stack.is_empty() && self.pat_binds_replacement(pat) {
                if let Some(scope) = self.scope_stack.last_mut() {
                    *scope = true;
                }
            }
        }

        fn visit_binding_pat_defaults(&mut self, pat: &Pat) {
            match pat {
                Pat::Array(array) => {
                    for elem in array.elems.iter().flatten() {
                        self.visit_binding_pat_defaults(elem);
                    }
                }
                Pat::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            ObjectPatProp::KeyValue(kv) => {
                                self.visit_binding_pat_defaults(&kv.value);
                            }
                            ObjectPatProp::Assign(assign) => {
                                if let Some(value) = &assign.value {
                                    value.visit_with(self);
                                }
                            }
                            ObjectPatProp::Rest(rest) => {
                                self.visit_binding_pat_defaults(&rest.arg);
                            }
                        }
                    }
                }
                Pat::Assign(assign) => {
                    assign.right.visit_with(self);
                    self.visit_binding_pat_defaults(&assign.left);
                }
                Pat::Rest(rest) => self.visit_binding_pat_defaults(&rest.arg),
                _ => {}
            }
        }
    }

    impl Visit for Checker<'_> {
        fn visit_import_decl(&mut self, _: &ImportDecl) {}

        fn visit_function(&mut self, function: &Function) {
            let params_shadow = function
                .params
                .iter()
                .any(|param| self.pat_binds_replacement(&param.pat));
            let body_shadow = function
                .body
                .as_ref()
                .is_some_and(|body| self.block_binds_replacement(body));
            self.scope_stack.push(params_shadow || body_shadow);

            for param in &function.params {
                self.visit_binding_pat_defaults(&param.pat);
            }
            if let Some(body) = &function.body {
                body.visit_with(self);
            }

            self.scope_stack.pop();
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            let params_shadow = arrow
                .params
                .iter()
                .any(|param| self.pat_binds_replacement(param));
            let body_shadow = match arrow.body.as_ref() {
                swc_core::ecma::ast::BlockStmtOrExpr::BlockStmt(body) => {
                    self.block_binds_replacement(body)
                }
                swc_core::ecma::ast::BlockStmtOrExpr::Expr(_) => false,
            };
            self.scope_stack.push(params_shadow || body_shadow);

            for param in &arrow.params {
                self.visit_binding_pat_defaults(param);
            }
            arrow.body.visit_with(self);

            self.scope_stack.pop();
        }

        fn visit_block_stmt(&mut self, block: &BlockStmt) {
            let body_shadow = self.block_binds_replacement(block);
            self.scope_stack.push(body_shadow);
            block.visit_children_with(self);
            self.scope_stack.pop();
        }

        fn visit_catch_clause(&mut self, catch: &CatchClause) {
            let param_shadow = catch
                .param
                .as_ref()
                .is_some_and(|param| self.pat_binds_replacement(param));
            let body_shadow = self.block_binds_replacement(&catch.body);
            self.scope_stack.push(param_shadow || body_shadow);

            if let Some(param) = &catch.param {
                self.visit_binding_pat_defaults(param);
            }
            catch.body.visit_with(self);

            self.scope_stack.pop();
        }

        fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
            self.mark_current_scope_binding(&declarator.name);
            self.visit_binding_pat_defaults(&declarator.name);
            if let Some(init) = &declarator.init {
                init.visit_with(self);
            }
        }

        fn visit_pat(&mut self, pat: &Pat) {
            self.visit_binding_pat_defaults(pat);
        }

        fn visit_decl(&mut self, decl: &Decl) {
            match decl {
                Decl::Fn(function) => function.function.visit_with(self),
                Decl::Class(class) => class.class.visit_with(self),
                _ => decl.visit_children_with(self),
            }
        }

        fn visit_class(&mut self, class: &Class) {
            class.visit_children_with(self);
        }

        fn visit_ident(&mut self, ident: &Ident) {
            if self.is_old_ident(ident) && self.in_shadowing_scope() {
                self.found = true;
            }
        }

        fn visit_prop_name(&mut self, _: &PropName) {}

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(computed) = prop {
                computed.visit_with(self);
            }
        }
    }

    let mut checker = Checker {
        old,
        replacement_name,
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

fn block_binds_name(block: &BlockStmt, name: &Atom) -> bool {
    struct Collector<'a> {
        name: &'a Atom,
        found: bool,
    }

    impl Collector<'_> {
        fn pat_binds_name(&self, pat: &Pat) -> bool {
            pat_binds_name(pat, self.name)
        }
    }

    impl Visit for Collector<'_> {
        fn visit_decl(&mut self, decl: &Decl) {
            match decl {
                Decl::Var(var) => {
                    if var.decls.iter().any(|decl| self.pat_binds_name(&decl.name)) {
                        self.found = true;
                    }
                    for decl in &var.decls {
                        if let Some(init) = &decl.init {
                            init.visit_with(self);
                        }
                    }
                }
                Decl::Fn(function) => {
                    if &function.ident.sym == self.name {
                        self.found = true;
                    }
                }
                Decl::Class(class) => {
                    if &class.ident.sym == self.name {
                        self.found = true;
                    }
                    class.class.visit_children_with(self);
                }
                _ => decl.visit_children_with(self),
            }
        }

        fn visit_function(&mut self, _: &Function) {}

        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

        fn visit_prop_name(&mut self, _: &PropName) {}

        fn visit_member_prop(&mut self, prop: &MemberProp) {
            if let MemberProp::Computed(prop) = prop {
                prop.visit_with(self);
            }
        }
    }

    let mut collector = Collector { name, found: false };
    block.visit_with(&mut collector);
    collector.found
}

fn pat_binds_name(pat: &Pat, name: &Atom) -> bool {
    match pat {
        Pat::Ident(id) => &id.id.sym == name,
        Pat::Array(arr) => arr.elems.iter().flatten().any(|p| pat_binds_name(p, name)),
        Pat::Object(obj) => obj.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => pat_binds_name(&kv.value, name),
            ObjectPatProp::Assign(assign) => &assign.key.id.sym == name,
            ObjectPatProp::Rest(rest) => pat_binds_name(&rest.arg, name),
        }),
        Pat::Assign(assign) => pat_binds_name(&assign.left, name),
        Pat::Rest(rest) => pat_binds_name(&rest.arg, name),
        _ => false,
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

pub(crate) struct BindingRenamer<'a> {
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

    /// Rename the local binding of a named import specifier while preserving the
    /// external (imported) name.  Without this override, renaming a shorthand
    /// specifier `import { createHash }` would produce `import { newName }` which
    /// tries to import `newName` from the module — wrong.  We instead emit
    /// `import { createHash as newName }`.
    fn visit_mut_import_named_specifier(&mut self, spec: &mut ImportNamedSpecifier) {
        for rename in self.renames {
            if spec.local.sym == rename.old.0 && spec.local.ctxt == rename.old.1 {
                // Lock in the external name before changing local.
                if spec.imported.is_none() {
                    spec.imported = Some(ModuleExportName::Ident(swc_core::ecma::ast::Ident::new(
                        spec.local.sym.clone(),
                        spec.local.span,
                        spec.local.ctxt,
                    )));
                }
                spec.local.sym = rename.new.clone();
                return;
            }
        }
    }

    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        if let Prop::Shorthand(ident) = prop {
            for rename in self.renames {
                if ident.sym == rename.old.0 && ident.ctxt == rename.old.1 {
                    let key = PropName::Ident(ident.clone().into());
                    ident.sym = rename.new.clone();
                    *prop = Prop::KeyValue(KeyValueProp {
                        key,
                        value: Box::new(Expr::Ident(ident.clone())),
                    });
                    return;
                }
            }
        }

        prop.visit_mut_children_with(self);
    }

    fn visit_mut_prop_name(&mut self, _: &mut PropName) {}

    fn visit_mut_member_prop(&mut self, prop: &mut MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_mut_with(self);
        }
    }
}
