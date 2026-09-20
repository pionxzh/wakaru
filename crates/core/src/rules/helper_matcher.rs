use crate::collections::HashSet;

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, Callee, Decl, Expr, Ident, ImportSpecifier, Lit, MemberProp, Module, ModuleItem,
    Pat, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub(crate) use crate::analysis::{
    binding_id as binding_key, ident_matches_binding, BindingId as BindingKey,
};

pub(crate) fn binding_key_from_ident_pat(pat: &Pat) -> Option<BindingKey> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(binding_key(&binding.id))
}

pub(crate) fn expr_matches_binding(expr: &Expr, key: &BindingKey) -> bool {
    matches!(expr, Expr::Ident(id) if ident_matches_binding(id, key))
}

pub(crate) fn expr_binding_key(expr: &Expr) -> Option<BindingKey> {
    let Expr::Ident(id) = expr else {
        return None;
    };
    Some(binding_key(id))
}

pub(crate) fn static_member_prop_name(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(id) => Some(id.sym.as_ref()),
        MemberProp::Computed(c) => match c.expr.as_ref() {
            Expr::Lit(Lit::Str(s)) => s.value.as_str(),
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

pub(crate) fn member_prop_name(prop: &MemberProp, name: &str) -> bool {
    static_member_prop_name(prop) == Some(name)
}

pub(crate) fn var_declarator_binding_key(decl: &VarDeclarator) -> Option<BindingKey> {
    binding_key_from_ident_pat(&decl.name)
}

pub(crate) fn import_specifier_binding_key(specifier: &ImportSpecifier) -> BindingKey {
    match specifier {
        ImportSpecifier::Default(default) => binding_key(&default.local),
        ImportSpecifier::Named(named) => binding_key(&named.local),
        ImportSpecifier::Namespace(namespace) => binding_key(&namespace.local),
    }
}

pub(crate) fn fn_decl_binding_key(item: &ModuleItem) -> Option<BindingKey> {
    let ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(Decl::Fn(fn_decl))) = item else {
        return None;
    };
    Some(binding_key(&fn_decl.ident))
}

/// Collect references to `targets`, skipping only var declarators whose binding
/// is in `skipped_decls`. This is useful for helper declarations that can share
/// a `var` statement with unrelated declarators.
pub(crate) fn remaining_refs_outside_var_declarators(
    module: &Module,
    targets: &HashSet<BindingKey>,
    skipped_decls: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let mut finder = VarDeclaratorSkippingRefFinder {
        targets,
        skipped_decls,
        skip_functions: false,
        found: HashSet::default(),
    };
    module.visit_with(&mut finder);
    finder.found
}

/// Collect references to `targets`, skipping function declarations and
/// individual var declarators whose bindings are in `skipped_decls`.
pub(crate) fn remaining_refs_outside_declarations<N>(
    node: &N,
    targets: &HashSet<BindingKey>,
    skipped_decls: &HashSet<BindingKey>,
) -> HashSet<BindingKey>
where
    N: for<'a> VisitWith<VarDeclaratorSkippingRefFinder<'a>> + ?Sized,
{
    let mut finder = VarDeclaratorSkippingRefFinder {
        targets,
        skipped_decls,
        skip_functions: true,
        found: HashSet::default(),
    };
    node.visit_with(&mut finder);
    finder.found
}
/// The candidates whose declarations can go: nothing outside the removable
/// set references them. A candidate that stays referenced keeps its
/// declaration, so the references inside it count for the others; the set
/// shrinks until it is stable instead of skipping every candidate declaration
/// once, which would remove a dependency of a kept candidate.
pub(crate) fn removable_without_remaining_refs<N>(
    module: &N,
    candidates: &HashSet<BindingKey>,
) -> HashSet<BindingKey>
where
    N: for<'a> VisitWith<VarDeclaratorSkippingRefFinder<'a>> + ?Sized,
{
    let mut removable = candidates.clone();
    loop {
        let remaining = remaining_refs_outside_declarations(module, &removable, &removable);
        if remaining.is_empty() {
            return removable;
        }
        removable.retain(|key| !remaining.contains(key));
    }
}

/// [`removable_without_remaining_refs`] for candidates that are only var
/// declarators: function declarations are not skipped.
pub(crate) fn removable_without_remaining_var_declarator_refs(
    module: &Module,
    candidates: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let mut removable = candidates.clone();
    loop {
        let remaining = remaining_refs_outside_var_declarators(module, &removable, &removable);
        if remaining.is_empty() {
            return removable;
        }
        removable.retain(|key| !remaining.contains(key));
    }
}

/// Collect which bindings from `targets` are referenced anywhere in `node`.
pub(crate) fn collect_refs<T>(node: &T, targets: &HashSet<BindingKey>) -> HashSet<BindingKey>
where
    for<'a> T: VisitWith<RemainingRefFinder<'a>>,
{
    let mut finder = RemainingRefFinder {
        targets,
        found: HashSet::default(),
    };
    node.visit_with(&mut finder);
    finder.found
}

/// Count how many times `key` is referenced anywhere in `node`.
pub(crate) fn count_binding_refs<T>(node: &T, key: &BindingKey) -> usize
where
    for<'a> T: VisitWith<SingleBindingRefCounter<'a>>,
{
    let mut counter = SingleBindingRefCounter { key, count: 0 };
    node.visit_with(&mut counter);
    counter.count
}

pub(crate) struct RemainingRefFinder<'a> {
    targets: &'a HashSet<BindingKey>,
    found: HashSet<BindingKey>,
}

impl Visit for RemainingRefFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.targets.contains(&key) {
            self.found.insert(key);
        }
    }
}

pub(crate) struct SingleBindingRefCounter<'a> {
    key: &'a BindingKey,
    count: usize,
}

impl Visit for SingleBindingRefCounter<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.key.0 && ident.ctxt == self.key.1 {
            self.count += 1;
        }
    }
}

pub(crate) struct VarDeclaratorSkippingRefFinder<'a> {
    targets: &'a HashSet<BindingKey>,
    skipped_decls: &'a HashSet<BindingKey>,
    skip_functions: bool,
    found: HashSet<BindingKey>,
}

impl Visit for VarDeclaratorSkippingRefFinder<'_> {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        if self.skip_functions && self.skipped_decls.contains(&binding_key(&decl.ident)) {
            return;
        }
        decl.visit_children_with(self);
    }

    fn visit_export_decl(&mut self, export: &swc_core::ecma::ast::ExportDecl) {
        // Direct exports remain observable even without a local reference.
        // Visit their binding patterns explicitly before skipping initializers.
        match &export.decl {
            Decl::Fn(decl) => decl.ident.visit_with(self),
            Decl::Var(var) => {
                for decl in &var.decls {
                    decl.name.visit_with(self);
                }
            }
            _ => {}
        }
        export.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if var_declarator_binding_key(decl)
            .as_ref()
            .is_some_and(|key| self.skipped_decls.contains(key))
        {
            return;
        }

        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_import_decl(&mut self, _: &swc_core::ecma::ast::ImportDecl) {}

    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.targets.contains(&key) {
            self.found.insert(key);
        }
    }
}

/// Remove unused, caller-proven helper declarations in any statement list.
/// Candidates must name function/variable declarations in statement lists;
/// their initializers must be safe to discard under the caller's helper proof.
/// This does not discover helpers, remove imports, or prove initializer purity.
/// Returns the removable set so callers can apply their own import policy.
/// Pass the whole module whenever available so sibling scopes and exports count.
pub(crate) fn remove_unused_helper_declarations<N>(
    node: &mut N,
    candidates: &HashSet<BindingKey>,
) -> HashSet<BindingKey>
where
    N: for<'a> VisitWith<VarDeclaratorSkippingRefFinder<'a>>
        + for<'a> VisitMutWith<HelperDeclarationRemover<'a>>,
{
    if candidates.is_empty() {
        return HashSet::default();
    }
    let removable = removable_without_remaining_refs(node, candidates);
    if !removable.is_empty() {
        node.visit_mut_with(&mut HelperDeclarationRemover {
            removable: &removable,
        });
    }
    removable
}

pub(crate) struct HelperDeclarationRemover<'a> {
    removable: &'a HashSet<BindingKey>,
}

impl HelperDeclarationRemover<'_> {
    fn retain_stmt(&self, stmt: &mut Stmt) -> bool {
        match stmt {
            Stmt::Decl(Decl::Fn(decl)) => !self.removable.contains(&binding_key(&decl.ident)),
            Stmt::Decl(Decl::Var(var)) => {
                var.decls.retain(|decl| {
                    var_declarator_binding_key(decl)
                        .is_none_or(|key| !self.removable.contains(&key))
                });
                !var.decls.is_empty()
            }
            _ => true,
        }
    }
}

impl VisitMut for HelperDeclarationRemover<'_> {
    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        items.retain_mut(|item| match item {
            ModuleItem::Stmt(stmt) => self.retain_stmt(stmt),
            _ => true,
        });
        items.visit_mut_children_with(self);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.retain_mut(|stmt| self.retain_stmt(stmt));
        stmts.visit_mut_children_with(self);
    }
}

pub(crate) fn remove_fn_decls_by_binding(module: &mut Module, removable: &HashSet<BindingKey>) {
    remove_fn_decls_from_body_by_binding(&mut module.body, removable);
}

pub(crate) fn remove_fn_decls_from_body_by_binding(
    body: &mut Vec<ModuleItem>,
    removable: &HashSet<BindingKey>,
) {
    body.retain(|item| fn_decl_binding_key(item).is_none_or(|key| !removable.contains(&key)));
}

pub(crate) fn remove_var_declarators_by_binding(
    body: &mut Vec<ModuleItem>,
    removable: &HashSet<BindingKey>,
) {
    for item in body.iter_mut() {
        let ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        var.decls.retain(|decl| {
            var_declarator_binding_key(decl).is_none_or(|key| !removable.contains(&key))
        });
    }
    body.retain(|item| {
        let ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(Decl::Var(var))) = item else {
            return true;
        };
        !var.decls.is_empty()
    });
}

pub(crate) fn remove_import_specifiers_by_binding(
    body: &mut Vec<ModuleItem>,
    removable: &HashSet<BindingKey>,
) {
    // An import that loses its last specifier here goes with it. A bare
    // `import "./side.js"` never had one and is a side effect the module
    // depends on; it stays.
    body.retain_mut(|item| {
        let ModuleItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::Import(import)) = item else {
            return true;
        };
        if import.specifiers.is_empty() {
            return true;
        }
        import
            .specifiers
            .retain(|specifier| !removable.contains(&import_specifier_binding_key(specifier)));
        !import.specifiers.is_empty()
    });
}

pub(crate) fn collect_import_binding_keys(module: &Module) -> HashSet<BindingKey> {
    let mut keys = HashSet::default();
    for item in &module.body {
        let ModuleItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::Import(import)) = item else {
            continue;
        };
        for spec in &import.specifiers {
            keys.insert(import_specifier_binding_key(spec));
        }
    }
    keys
}

/// Top-level `var x = require(<number>)` declarations. Bundlers rewrite
/// `@swc/helpers` imports into numeric module ids, so the object-spread and
/// object-rest rules treat every such declaration as a *candidate* helper
/// namespace and match member calls through it.
///
/// The paired sweep may delete only candidates the calling rule's own
/// rewrites orphaned — referenced when `collect` ran, unreferenced at sweep
/// time. A candidate that was already unreferenced at entry was orphaned by
/// some earlier rule and must survive: its require call still carries a
/// module side effect, and the numeric id is the user's join key for chunks
/// missing from the input.
pub(crate) struct NumericRequireNamespaces {
    pub(crate) candidates: HashSet<BindingKey>,
    referenced_at_entry: HashSet<BindingKey>,
}

impl NumericRequireNamespaces {
    /// Only calls to the unresolved `require` qualify. Without a mark
    /// (`None`) there is no way to tell a global `require` from a shadowed
    /// one, so nothing is collected and the sweep is a no-op — matching by
    /// bare name would violate the SyntaxContext rule.
    pub(crate) fn collect(module: &Module, unresolved_mark: Option<Mark>) -> Self {
        let Some(unresolved_mark) = unresolved_mark else {
            return Self {
                candidates: HashSet::default(),
                referenced_at_entry: HashSet::default(),
            };
        };
        let mut candidates = HashSet::default();
        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            for decl in &var.decls {
                let Pat::Ident(binding) = &decl.name else {
                    continue;
                };
                if decl
                    .init
                    .as_deref()
                    .is_some_and(|init| is_numeric_require_call(init, unresolved_mark))
                {
                    candidates.insert(binding_key(&binding.id));
                }
            }
        }
        let referenced_at_entry =
            remaining_refs_outside_declarations(module, &candidates, &candidates);
        Self {
            candidates,
            referenced_at_entry,
        }
    }

    /// Remove candidate declarations orphaned by the calling rule's rewrites.
    pub(crate) fn sweep_orphaned(&self, body: &mut Vec<ModuleItem>) {
        let mut unused = HashSet::default();
        for key in &self.referenced_at_entry {
            let ident = Ident::new(key.0.clone(), DUMMY_SP, key.1);
            if !ident_used_in_items(body, &ident) {
                unused.insert(key.clone());
            }
        }
        if unused.is_empty() {
            return;
        }
        body.retain_mut(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                return true;
            };
            var.decls.retain(|decl| {
                let Pat::Ident(binding) = &decl.name else {
                    return true;
                };
                !unused.contains(&binding_key(&binding.id))
            });
            !var.decls.is_empty()
        });
    }
}

fn is_numeric_require_call(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require" && id.ctxt.outer() == unresolved_mark)
    {
        return false;
    }
    matches!(call.args[0].expr.as_ref(), Expr::Lit(Lit::Num(_)))
}

fn ident_used_in_items(body: &[ModuleItem], target: &Ident) -> bool {
    struct Finder<'a> {
        target: &'a Ident,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

        fn visit_ident(&mut self, ident: &Ident) {
            if ident.sym == self.target.sym && ident.ctxt == self.target.ctxt {
                self.found = true;
            }
        }
    }

    let mut finder = Finder {
        target,
        found: false,
    };
    for item in body {
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    finder.found
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::atoms::Atom;
    use swc_core::common::{SyntaxContext, DUMMY_SP, GLOBALS};
    use swc_core::ecma::ast::IdentName;

    fn ident(sym: &str, ctxt: SyntaxContext) -> Ident {
        Ident {
            span: DUMMY_SP,
            ctxt,
            sym: Atom::from(sym),
            optional: false,
        }
    }

    #[test]
    fn binding_match_checks_syntax_context() {
        GLOBALS.set(&Default::default(), || {
            let key = (
                Atom::from("a"),
                SyntaxContext::empty().apply_mark(swc_core::common::Mark::new()),
            );
            let expr = Expr::Ident(ident("a", SyntaxContext::empty()));
            assert!(!expr_matches_binding(&expr, &key));
        });
    }

    #[test]
    fn member_prop_name_accepts_ident_and_string_literal() {
        GLOBALS.set(&Default::default(), || {
            let ident_prop = MemberProp::Ident(IdentName {
                span: DUMMY_SP,
                sym: Atom::from("default"),
            });
            assert!(member_prop_name(&ident_prop, "default"));

            let computed_prop = MemberProp::Computed(swc_core::ecma::ast::ComputedPropName {
                span: DUMMY_SP,
                expr: Box::new(Expr::Lit(Lit::Str(swc_core::ecma::ast::Str {
                    span: DUMMY_SP,
                    value: "default".into(),
                    raw: None,
                }))),
            });
            assert!(member_prop_name(&computed_prop, "default"));
        });
    }

    fn cleanup_helpers(source: &str) -> Vec<String> {
        use swc_core::common::{sync::Lrc, FileName, SourceMap};
        use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
        use swc_core::ecma::transforms::base::resolver;
        use swc_core::ecma::visit::VisitMutWith;

        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), source.to_owned());
            let lexer = Lexer::new(
                Syntax::Es(EsSyntax::default()),
                Default::default(),
                StringInput::from(&*fm),
                None,
            );
            let mut module = Parser::new_from(lexer).parse_module().unwrap();
            module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
            #[derive(Default)]
            struct Declarations {
                helpers: HashSet<BindingKey>,
                names: Vec<String>,
            }
            impl Visit for Declarations {
                fn visit_binding_ident(&mut self, ident: &BindingIdent) {
                    self.names.push(ident.id.sym.to_string());
                    if ident.id.sym.starts_with("helper") {
                        self.helpers.insert(binding_key(&ident.id));
                    }
                }
                fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
                    self.names.push(decl.ident.sym.to_string());
                    if decl.ident.sym.starts_with("helper") {
                        self.helpers.insert(binding_key(&decl.ident));
                    }
                    decl.function.visit_with(self);
                }
            }
            let mut before = Declarations::default();
            module.visit_with(&mut before);
            remove_unused_helper_declarations(&mut module, &before.helpers);
            let mut after = Declarations::default();
            module.visit_with(&mut after);
            after.names.sort();
            after.names
        })
    }

    #[test]
    fn unused_helper_cycles_are_removed_from_nested_statement_lists() {
        let names = cleanup_helpers(
            r#"
            function outer() {
                function helperA() { helperB(); }
                function helperB() { helperA(); }
                var helperC = () => helperC(), keep = effect();
                { function helperBlock() {} }
                const arrow = () => { function helperArrow() {} };
                class C {
                    method() { function helperMethod() {} }
                    static { function helperStatic() {} }
                }
            }
        "#,
        );
        assert!(
            !names.iter().any(|name| name.starts_with("helper")),
            "{names:?}"
        );
        assert!(names.contains(&"keep".to_owned()));
        assert!(names.contains(&"outer".to_owned()));
    }

    #[test]
    fn surviving_helper_keeps_its_transitive_dependencies() {
        let names = cleanup_helpers(
            r#"
            function helperA() { helperB(); }
            function helperB() { helperC(); }
            function helperC() {}
            export { helperA };
        "#,
        );
        assert_eq!(names, ["helperA", "helperB", "helperC"]);
    }

    #[test]
    fn sibling_closures_and_computed_keys_keep_helpers_alive() {
        let names = cleanup_helpers(
            r#"
            function helperClosure() {}
            function helperKey() {}
            function reader() { return helperClosure; }
            export const value = { [helperKey()]: 1 };
        "#,
        );
        assert!(names.contains(&"helperClosure".to_owned()));
        assert!(names.contains(&"helperKey".to_owned()));
    }

    #[test]
    fn exported_declarations_keep_their_dependencies() {
        let names = cleanup_helpers(
            r#"
            export function helperExport() { helperDep(); }
            function helperDep() {}
            export const helperVar = () => helperVarDep();
            function helperVarDep() {}
        "#,
        );
        assert_eq!(
            names,
            ["helperDep", "helperExport", "helperVar", "helperVarDep"]
        );
    }

    #[test]
    fn same_spelling_in_a_different_scope_does_not_keep_a_helper_alive() {
        let names = cleanup_helpers(
            r#"
            function helper() {}
            function reader(helper) { return helper; }
        "#,
        );
        assert_eq!(names, ["helper", "reader"]);
    }
}
