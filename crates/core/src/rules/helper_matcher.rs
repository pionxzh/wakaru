use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{
    Callee, Decl, Expr, Ident, Lit, MemberProp, Module, ModuleItem, Pat, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) type BindingKey = (Atom, SyntaxContext);

pub(crate) fn binding_key(ident: &Ident) -> BindingKey {
    (ident.sym.clone(), ident.ctxt)
}

#[allow(dead_code)]
pub(crate) fn binding_key_from_ident_pat(pat: &Pat) -> Option<BindingKey> {
    let Pat::Ident(binding) = pat else {
        return None;
    };
    Some(binding_key(&binding.id))
}

#[allow(dead_code)]
pub(crate) fn ident_matches_binding(ident: &Ident, key: &BindingKey) -> bool {
    ident.sym == key.0 && ident.ctxt == key.1
}

#[allow(dead_code)]
pub(crate) fn expr_matches_binding(expr: &Expr, key: &BindingKey) -> bool {
    matches!(expr, Expr::Ident(id) if ident_matches_binding(id, key))
}

#[allow(dead_code)]
pub(crate) fn callee_matches_binding(callee: &Callee, key: &BindingKey) -> bool {
    let Callee::Expr(expr) = callee else {
        return false;
    };
    expr_matches_binding(expr, key)
}

#[allow(dead_code)]
pub(crate) fn member_prop_name(prop: &MemberProp, name: &str) -> bool {
    match prop {
        MemberProp::Ident(id) => id.sym.as_ref() == name,
        MemberProp::Computed(computed) => {
            matches!(
                computed.expr.as_ref(),
                Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some(name)
            )
        }
        MemberProp::PrivateName(_) => false,
    }
}

#[allow(dead_code)]
pub(crate) fn member_of_binding<'a>(
    expr: &'a Expr,
    key: &BindingKey,
    prop_name: &str,
) -> Option<&'a swc_core::ecma::ast::MemberExpr> {
    let Expr::Member(member) = expr else {
        return None;
    };
    if !expr_matches_binding(&member.obj, key) {
        return None;
    }
    member_prop_name(&member.prop, prop_name).then_some(member)
}

#[allow(dead_code)]
pub(crate) fn var_declarator_binding_key(decl: &VarDeclarator) -> Option<BindingKey> {
    binding_key_from_ident_pat(&decl.name)
}

pub(crate) fn fn_decl_binding_key(item: &ModuleItem) -> Option<BindingKey> {
    let ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(Decl::Fn(fn_decl))) = item else {
        return None;
    };
    Some(binding_key(&fn_decl.ident))
}

/// Collect references to `targets`, skipping module items that declare helpers
/// we are considering removable. This avoids counting a helper's own binding
/// name or self-references as external uses that pin the declaration.
pub(crate) fn remaining_refs_outside_skipped_items<F>(
    module: &Module,
    targets: &HashSet<BindingKey>,
    should_skip_item: F,
) -> HashSet<BindingKey>
where
    F: Fn(&ModuleItem) -> bool,
{
    let mut finder = RemainingRefFinder {
        targets,
        found: HashSet::new(),
    };
    for item in &module.body {
        if should_skip_item(item) {
            continue;
        }
        item.visit_with(&mut finder);
    }
    finder.found
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

struct RemainingRefFinder<'a> {
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

    fn visit_ident(&mut self, ident: &Ident) {
        let key = binding_key(ident);
        if self.targets.contains(&key) {
            self.found.insert(key);
        }
    }
}

pub(crate) fn remove_fn_decls_by_binding(module: &mut Module, removable: &HashSet<BindingKey>) {
    module
        .body
        .retain(|item| fn_decl_binding_key(item).is_none_or(|key| !removable.contains(&key)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::atoms::Atom;
    use swc_core::common::{SyntaxContext, DUMMY_SP, GLOBALS};
    use swc_core::ecma::ast::{IdentName, MemberExpr};

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

    #[test]
    fn member_of_binding_requires_matching_object_context() {
        GLOBALS.set(&Default::default(), || {
            let key = (Atom::from("obj"), SyntaxContext::empty());
            let member = Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(ident("obj", SyntaxContext::empty()))),
                prop: MemberProp::Ident(IdentName {
                    span: DUMMY_SP,
                    sym: Atom::from("prop"),
                }),
            });
            assert!(member_of_binding(&member, &key, "prop").is_some());

            let wrong_key = (
                Atom::from("obj"),
                SyntaxContext::empty().apply_mark(swc_core::common::Mark::new()),
            );
            assert!(member_of_binding(&member, &wrong_key, "prop").is_none());
        });
    }
}
