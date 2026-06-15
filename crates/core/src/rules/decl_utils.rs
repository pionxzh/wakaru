use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{BindingIdent, Decl, Id, Ident, Pat, Stmt, VarDecl, VarDeclKind};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitWith};

pub type BindingId = (Atom, SyntaxContext);

pub fn binding_id(ident: &Ident) -> BindingId {
    (ident.sym.clone(), ident.ctxt)
}

pub fn ident_matches_binding(ident: &Ident, binding: &BindingId) -> bool {
    ident.sym == binding.0 && ident.ctxt == binding.1
}

pub fn same_ident(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

/// Collect all binding names declared by a `Decl` (top-level only, does not
/// recurse into function bodies). Handles all destructuring forms via SWC's
/// `find_pat_ids`.
pub fn collect_decl_names(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => collect_var_decl_names(var, names),
        Decl::Fn(f) => {
            names.insert(f.ident.sym.clone());
        }
        Decl::Class(c) => {
            names.insert(c.ident.sym.clone());
        }
        _ => {}
    }
}

/// Collect all binding names from a `VarDecl`.
pub fn collect_var_decl_names(var: &VarDecl, names: &mut HashSet<Atom>) {
    for declarator in &var.decls {
        let ids: Vec<Id> = find_pat_ids(&declarator.name);
        names.extend(ids.into_iter().map(|(sym, _)| sym));
    }
}

/// Collect all binding `(Atom, SyntaxContext)` pairs declared by a `Decl`
/// (top-level only). Handles all destructuring forms.
pub fn collect_decl_binding_ids(decl: &Decl, ids: &mut HashSet<BindingId>) {
    match decl {
        Decl::Var(var) => collect_var_decl_binding_ids(var, ids),
        Decl::Fn(f) => {
            ids.insert(binding_id(&f.ident));
        }
        Decl::Class(c) => {
            ids.insert(binding_id(&c.ident));
        }
        _ => {}
    }
}

/// Collect all binding ids from a `VarDecl`.
pub fn collect_var_decl_binding_ids(var: &VarDecl, ids: &mut HashSet<BindingId>) {
    for declarator in &var.decls {
        let pat_ids: Vec<Id> = find_pat_ids(&declarator.name);
        ids.extend(pat_ids);
    }
}

/// Collect all binding names from a pattern. Delegates to SWC's `find_pat_ids`
/// which correctly handles `Ident`, `Array`, `Object` (key-value, assign, rest),
/// `Rest`, and `Assign` patterns without recursing into expressions.
pub fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    let ids: Vec<Id> = find_pat_ids(pat);
    names.extend(ids.into_iter().map(|(sym, _)| sym));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UninitializedDeclKind {
    Any,
    VarOnly,
}

pub(crate) fn can_remove_prior_uninitialized_decls(
    stmts: &[Stmt],
    targets: &[Ident],
    kind: UninitializedDeclKind,
) -> bool {
    can_remove_prior_uninitialized_decls_by(stmts, targets, kind, same_ident)
}

pub(crate) fn can_remove_prior_uninitialized_decls_by<F>(
    stmts: &[Stmt],
    targets: &[Ident],
    kind: UninitializedDeclKind,
    matches_ident: F,
) -> bool
where
    F: Fn(&Ident, &Ident) -> bool + Copy,
{
    if targets
        .iter()
        .any(|target| ident_is_used_in_stmts_excluding_bindings_by(target, stmts, matches_ident))
    {
        return false;
    }

    targets
        .iter()
        .all(|target| has_uninitialized_decl_by(stmts, target, kind, matches_ident))
}

pub(crate) fn remove_prior_uninitialized_decls(
    stmts: &mut Vec<Stmt>,
    end: usize,
    targets: &[Ident],
    kind: UninitializedDeclKind,
) {
    remove_prior_uninitialized_decls_by(stmts, end, targets, kind, same_ident);
}

pub(crate) fn remove_prior_uninitialized_decls_by<F>(
    stmts: &mut Vec<Stmt>,
    end: usize,
    targets: &[Ident],
    kind: UninitializedDeclKind,
    matches_ident: F,
) where
    F: Fn(&Ident, &Ident) -> bool + Copy,
{
    let end = end.min(stmts.len());
    for stmt in &mut stmts[..end] {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if kind == UninitializedDeclKind::VarOnly && var.kind != VarDeclKind::Var {
            continue;
        }
        var.decls.retain(|decl| {
            if decl.init.is_some() {
                return true;
            }
            let Pat::Ident(binding) = &decl.name else {
                return true;
            };
            !targets
                .iter()
                .any(|target| matches_ident(&binding.id, target))
        });
    }

    stmts.retain(|stmt| !matches!(stmt, Stmt::Decl(Decl::Var(var)) if var.decls.is_empty()));
}

pub(crate) fn ident_is_used_in_stmts_excluding_bindings(target: &Ident, stmts: &[Stmt]) -> bool {
    ident_is_used_in_stmts_excluding_bindings_by(target, stmts, same_ident)
}

pub(crate) fn ident_is_used_in_stmts_excluding_bindings_by<F>(
    target: &Ident,
    stmts: &[Stmt],
    matches_ident: F,
) -> bool
where
    F: Fn(&Ident, &Ident) -> bool + Copy,
{
    struct UseFinder<'a, F>
    where
        F: Fn(&Ident, &Ident) -> bool + Copy,
    {
        target: &'a Ident,
        matches_ident: F,
        found: bool,
    }

    impl<F> Visit for UseFinder<'_, F>
    where
        F: Fn(&Ident, &Ident) -> bool + Copy,
    {
        fn visit_binding_ident(&mut self, _: &BindingIdent) {}

        fn visit_ident(&mut self, ident: &Ident) {
            if (self.matches_ident)(ident, self.target) {
                self.found = true;
            }
        }
    }

    let mut finder = UseFinder {
        target,
        matches_ident,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

fn has_uninitialized_decl_by<F>(
    stmts: &[Stmt],
    target: &Ident,
    kind: UninitializedDeclKind,
    matches_ident: F,
) -> bool
where
    F: Fn(&Ident, &Ident) -> bool + Copy,
{
    stmts.iter().any(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return false;
        };
        if kind == UninitializedDeclKind::VarOnly && var.kind != VarDeclKind::Var {
            return false;
        }
        var.decls.iter().any(|decl| {
            decl.init.is_none()
                && matches!(&decl.name, Pat::Ident(binding) if matches_ident(&binding.id, target))
        })
    })
}
