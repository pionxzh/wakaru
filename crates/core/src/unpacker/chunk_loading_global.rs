//! The global array a chunk-loading runtime installs and its chunks push into
//! (`self.webpackChunk_app`, `window.webpackJsonp`, `globalThis.TURBOPACK`).
//!
//! Every chunk of one build registers its factories through the array its
//! runtime created, so the array's name identifies the build. One page often
//! loads several unrelated builds, each with its own module table; multi-input
//! unpack uses these names to keep a numeric `require(<id>)` from linking into
//! another build's module that happens to carry the same id.

use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinaryOp, Callee, Expr, ExprStmt, MemberExpr, MemberProp,
    Module, ModuleItem, SimpleAssignTarget, Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::utils::paren::strip_parens;

/// Names of the chunk-loading globals this input pushes into, or, for an input
/// that pushes into none, the ones it binds as a runtime. Sorted and deduplicated;
/// empty when the input shows neither.
///
/// Pushes count only as top-level statements, the position chunk detection
/// reads, so an analytics snippet inside a factory
/// (`(window.adsbygoogle = window.adsbygoogle || []).push({})`) is not taken
/// for a chunk. A runtime binds the array to a local before replacing its
/// `push` (`var n = self.webpackChunk_app = self.webpackChunk_app || []`); a
/// bare `window.dataLayer = window.dataLayer || []` statement does not count.
pub(crate) fn chunk_loading_globals(module: &Module) -> Vec<String> {
    let mut names = module
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) => pushed_global(expr),
            _ => None,
        })
        .collect::<Vec<_>>();
    if names.is_empty() {
        let mut collector = RuntimeBindingCollector::default();
        module.visit_with(&mut collector);
        names = collector.names;
    }
    names.sort();
    names.dedup();
    names
}

/// `(G = G || []).push(...)` (webpack) or `(G || (G = [])).push(...)` (Turbopack).
fn pushed_global(expr: &Expr) -> Option<String> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "push") {
        return None;
    }
    initialized_global(&member.obj).or_else(|| guarded_global(&member.obj))
}

#[derive(Default)]
struct RuntimeBindingCollector {
    names: Vec<String>,
}

impl Visit for RuntimeBindingCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let Some(name) = declarator.init.as_deref().and_then(initialized_global) {
            self.names.push(name);
        }
        declarator.visit_children_with(self);
    }

    // A minifier may hoist the local and bind it by assignment:
    // `r = self.webpackChunk_app = self.webpackChunk_app || []`.
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if assign.op == AssignOp::Assign {
            if let Some(name) = initialized_global(&assign.right) {
                self.names.push(name);
            }
        }
        assign.visit_children_with(self);
    }
}

/// `G = G || []`.
fn initialized_global(expr: &Expr) -> Option<String> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return None;
    };
    let Expr::Bin(fallback) = strip_parens(&assign.right) else {
        return None;
    };
    if fallback.op != BinaryOp::LogicalOr || !is_empty_array(&fallback.right) {
        return None;
    }
    let Expr::Member(read) = strip_parens(&fallback.left) else {
        return None;
    };
    same_global(target, read)
}

/// `G || (G = [])`.
fn guarded_global(expr: &Expr) -> Option<String> {
    let Expr::Bin(guard) = strip_parens(expr) else {
        return None;
    };
    if guard.op != BinaryOp::LogicalOr {
        return None;
    }
    let Expr::Member(read) = strip_parens(&guard.left) else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(&guard.right) else {
        return None;
    };
    if assign.op != AssignOp::Assign || !is_empty_array(&assign.right) {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return None;
    };
    same_global(target, read)
}

/// The property name when both members read the same property of the same
/// object (`self`, `window`, `globalThis`, a parameter, or `this`).
fn same_global(left: &MemberExpr, right: &MemberExpr) -> Option<String> {
    let name = static_prop_name(&left.prop)?;
    if static_prop_name(&right.prop)? != name {
        return None;
    }
    let same_object = match (strip_parens(&left.obj), strip_parens(&right.obj)) {
        (Expr::Ident(left), Expr::Ident(right)) => left.to_id() == right.to_id(),
        (Expr::This(_), Expr::This(_)) => true,
        _ => false,
    };
    same_object.then_some(name)
}

fn static_prop_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(name) => Some(name.sym.to_string()),
        MemberProp::Computed(computed) => match strip_parens(&computed.expr) {
            Expr::Lit(swc_core::ecma::ast::Lit::Str(name)) => {
                name.value.as_str().map(str::to_string)
            }
            _ => None,
        },
        MemberProp::PrivateName(_) => None,
    }
}

fn is_empty_array(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Array(array) if array.elems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};

    fn globals(source: &str) -> Vec<String> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "input.js", cm)
                .expect("fixture should parse");
            chunk_loading_globals(&module)
        })
    }

    #[test]
    fn chunk_pushes_name_their_global() {
        assert_eq!(
            globals("(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {}]);"),
            ["webpackChunk_app"]
        );
        assert_eq!(
            globals(r#"(window["webpackJsonp"] = window["webpackJsonp"] || []).push([[0], []]);"#),
            ["webpackJsonp"]
        );
        assert_eq!(
            globals(
                r#"(globalThis.TURBOPACK || (globalThis.TURBOPACK = [])).push(["s.js", 1, t => {}]);"#
            ),
            ["TURBOPACK"]
        );
        assert_eq!(
            globals(
                "(this.webpackChunk_a = this.webpackChunk_a || []).push([[1], {}]);\n\
                 (this.webpackChunk_a = this.webpackChunk_a || []).push([[2], {}]);"
            ),
            ["webpackChunk_a"]
        );
    }

    #[test]
    fn runtimes_name_the_global_they_bind() {
        assert_eq!(
            globals(
                "!function(e){var u=window.webpackJsonp=window.webpackJsonp||[],i=u.push.bind(u);u.push=e}(0);"
            ),
            ["webpackJsonp"]
        );
        assert_eq!(
            globals(
                r#"(() => { const g = self["webpackChunk"] = self["webpackChunk"] || []; })();"#
            ),
            ["webpackChunk"]
        );
        assert_eq!(
            globals(
                "(() => { var n; n = self.webpackChunk_N_E = self.webpackChunk_N_E || []; })();"
            ),
            ["webpackChunk_N_E"]
        );
    }

    #[test]
    fn unrelated_arrays_do_not_name_a_build() {
        assert!(globals("window.dataLayer = window.dataLayer || [];").is_empty());
        assert!(globals(
            "(self.webpackChunk_app = self.webpackChunk_app || []).push([[1], {\n\
               1: function() { (window.adsbygoogle = window.adsbygoogle || []).push({}); }\n\
             }]);"
        )
        .iter()
        .eq(["webpackChunk_app"].iter()));
        assert!(globals("var q = window.a.b = window.c.b || [];").is_empty());
        assert!(globals("(self.x = self.x || [1]).push(2);").is_empty());
    }
}
