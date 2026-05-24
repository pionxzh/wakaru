use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::SyntaxContext;
use swc_core::ecma::ast::{Decl, Id, Ident, Pat, VarDecl};
use swc_core::ecma::utils::find_pat_ids;

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
