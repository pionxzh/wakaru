use std::collections::HashSet;

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, Callee, Decl, Expr, Ident, ImportSpecifier, Lit, MemberProp, Module, ModuleItem,
    Pat, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

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
        found: HashSet::new(),
    };
    module.visit_with(&mut finder);
    finder.found
}

/// Collect references to `targets`, skipping function declarations and
/// individual var declarators whose bindings are in `skipped_decls`.
pub(crate) fn remaining_refs_outside_declarations(
    module: &Module,
    targets: &HashSet<BindingKey>,
    skipped_decls: &HashSet<BindingKey>,
) -> HashSet<BindingKey> {
    let mut finder = VarDeclaratorSkippingRefFinder {
        targets,
        skipped_decls,
        found: HashSet::new(),
    };

    for item in &module.body {
        if fn_decl_binding_key(item)
            .as_ref()
            .is_some_and(|key| skipped_decls.contains(key))
        {
            continue;
        }
        item.visit_with(&mut finder);
    }

    finder.found
}

/// Collect which bindings from `targets` are referenced anywhere in `node`.
pub(crate) fn collect_refs<T>(node: &T, targets: &HashSet<BindingKey>) -> HashSet<BindingKey>
where
    for<'a> T: VisitWith<RemainingRefFinder<'a>>,
{
    let mut finder = RemainingRefFinder {
        targets,
        found: HashSet::new(),
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

struct VarDeclaratorSkippingRefFinder<'a> {
    targets: &'a HashSet<BindingKey>,
    skipped_decls: &'a HashSet<BindingKey>,
    found: HashSet<BindingKey>,
}

impl Visit for VarDeclaratorSkippingRefFinder<'_> {
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
    for item in body.iter_mut() {
        let ModuleItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::Import(import)) = item else {
            continue;
        };
        import
            .specifiers
            .retain(|specifier| !removable.contains(&import_specifier_binding_key(specifier)));
    }
    body.retain(|item| {
        let ModuleItem::ModuleDecl(swc_core::ecma::ast::ModuleDecl::Import(import)) = item else {
            return true;
        };
        !import.specifiers.is_empty()
    });
}

pub(crate) fn collect_import_binding_keys(module: &Module) -> HashSet<BindingKey> {
    let mut keys = HashSet::new();
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
    /// With `Some(mark)`, only calls to the unresolved `require` qualify;
    /// `None` accepts any `require` identifier (UnObjectSpread's behavior
    /// when constructed without a mark).
    pub(crate) fn collect(module: &Module, unresolved_mark: Option<Mark>) -> Self {
        let mut candidates = HashSet::new();
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
        let mut unused = HashSet::new();
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

fn is_numeric_require_call(expr: &Expr, unresolved_mark: Option<Mark>) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !matches!(callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "require" && unresolved_mark.is_none_or(|mark| id.ctxt.outer() == mark))
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
}
