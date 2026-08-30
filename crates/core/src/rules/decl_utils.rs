use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    BindingIdent, Decl, Expr, Function, FunctionBody, Id, Ident, Lit, MethodKind, ObjectLit, Param,
    Pat, Prop, PropName, PropOrSpread, Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

pub(crate) use crate::analysis::{binding_id, ident_matches_binding, BindingId};

pub fn same_ident(left: &Ident, right: &Ident) -> bool {
    left.sym == right.sym && left.ctxt == right.ctxt
}

/// Whether the function's directive prologue directly enables strict mode.
///
/// A function with this directive must keep a simple parameter list. Rewrites
/// that synthesize defaults, rest parameters, or destructuring must therefore
/// leave its parameters and prologue statements untouched.
pub(crate) fn has_direct_use_strict_directive(body: &FunctionBody) -> bool {
    for stmt in &body.stmts {
        let Stmt::Expr(expr_stmt) = stmt else {
            break;
        };
        let Expr::Lit(Lit::Str(directive)) = expr_stmt.expr.as_ref() else {
            break;
        };
        if directive.value.as_str() == Some("use strict") {
            return true;
        }
    }
    false
}

/// Whether the direct statement list contains a `"use strict"` string that a
/// rewrite could promote into the directive prologue by removing its prefix.
pub(crate) fn contains_use_strict_string_statement(body: &FunctionBody) -> bool {
    body.stmts.iter().any(|stmt| {
        matches!(stmt, Stmt::Expr(expr_stmt)
            if matches!(expr_stmt.expr.as_ref(), Expr::Lit(Lit::Str(value))
                if value.value.as_str() == Some("use strict")))
    })
}

/// Whether a parameter list declares the same name twice. Arrow, method, and
/// class-method parameter lists reject duplicates as an early error, so
/// converting a sloppy-mode function that carries them must be skipped.
/// Duplicates are only legal in a simple (all-identifier) parameter list, so
/// checking `Pat::Ident` syms is exhaustive.
pub fn has_duplicate_param_names(params: &[Param]) -> bool {
    let mut seen = HashSet::new();
    params.iter().any(
        |param| matches!(&param.pat, Pat::Ident(binding) if !seen.insert(binding.id.sym.clone())),
    )
}

/// Add the required value parameter to a zero-argument function that is being
/// recovered as a class setter. The name must not capture any existing
/// reference when the callback body moves into method scope.
pub(crate) fn ensure_setter_has_value_param(function: &mut Function) {
    if !function.params.is_empty() {
        return;
    }
    let name = dummy_setter_param_name(function);
    function.params.push(Param {
        span: DUMMY_SP,
        decorators: vec![],
        pat: Pat::Ident(BindingIdent {
            id: Ident::new(name, DUMMY_SP, SyntaxContext::empty()),
            type_ann: None,
        }),
    });
}

/// Whether cloning a function into a class method of `kind` would create an
/// invalid accessor or a strict-mode duplicate parameter list.
pub(crate) fn class_method_has_invalid_signature(function: &Function, kind: MethodKind) -> bool {
    if has_duplicate_param_names(&function.params) {
        return true;
    }
    match kind {
        MethodKind::Getter => {
            !function.params.is_empty() || function.is_async || function.is_generator
        }
        MethodKind::Setter => {
            function.params.len() != 1
                || matches!(&function.params[0].pat, Pat::Rest(_))
                || function.is_async
                || function.is_generator
        }
        MethodKind::Method => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClassAccessorDescriptorAttributes {
    ClassCompatible,
    Enumerable,
}

/// Classify exact accessor descriptors that can be traced back to class
/// syntax. Direct `Object.defineProperty` defaults `configurable` to false,
/// while class accessors are configurable and non-enumerable. TypeScript
/// 3.5–3.8 is the known producer of the enumerable variant.
pub(crate) fn class_accessor_descriptor_attributes(
    descriptor: &ObjectLit,
) -> Option<ClassAccessorDescriptorAttributes> {
    let mut seen = HashSet::new();
    let mut has_accessor = false;
    let mut configurable = false;
    let mut enumerable = false;

    for prop in &descriptor.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(key_value) => {
                let name = static_prop_name(&key_value.key)?;
                if !seen.insert(name.clone()) {
                    return None;
                }
                match name.as_ref() {
                    "get" | "set" => {
                        if !matches!(strip_parens(&key_value.value), Expr::Fn(_)) {
                            return None;
                        }
                        has_accessor = true;
                    }
                    "configurable" => {
                        if !matches!(strip_parens(&key_value.value), Expr::Lit(Lit::Bool(value)) if value.value)
                        {
                            return None;
                        }
                        configurable = true;
                    }
                    "enumerable" => {
                        let Expr::Lit(Lit::Bool(value)) = strip_parens(&key_value.value) else {
                            return None;
                        };
                        enumerable = value.value;
                    }
                    _ => return None,
                }
            }
            Prop::Method(method) => {
                let name = static_prop_name(&method.key)?;
                if !matches!(name.as_ref(), "get" | "set") || !seen.insert(name) {
                    return None;
                }
                has_accessor = true;
            }
            _ => return None,
        }
    }

    if !has_accessor || !configurable {
        return None;
    }
    Some(if enumerable {
        ClassAccessorDescriptorAttributes::Enumerable
    } else {
        ClassAccessorDescriptorAttributes::ClassCompatible
    })
}

fn static_prop_name(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(string) => Some(string.value.as_str()?.into()),
        _ => None,
    }
}

fn dummy_setter_param_name(function: &Function) -> Atom {
    struct Collector<'a> {
        names: &'a mut HashSet<Atom>,
    }

    impl Visit for Collector<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            self.names.insert(ident.sym.clone());
        }
    }

    let mut names = HashSet::new();
    function.visit_with(&mut Collector { names: &mut names });
    for candidate in ["_", "_v", "_0", "_1", "_2"] {
        let atom: Atom = candidate.into();
        if !names.contains(&atom) {
            return atom;
        }
    }
    let mut index = 3u32;
    loop {
        let atom: Atom = format!("_{index}").into();
        if !names.contains(&atom) {
            return atom;
        }
        index += 1;
    }
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
