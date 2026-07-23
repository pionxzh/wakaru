//! Container syntax shared by webpack 4 and webpack 5 output.
//!
//! Both major versions render module tables through the same template
//! (`Template.getModulesArrayBounds`): a sparse array when module ids are
//! dense numerics, wrapped in `Array(minId).concat([...])` when the smallest
//! id is non-zero. The matchers live here so neither version's unpacker owns
//! the other's syntax.

use swc_core::ecma::ast::{ArrayLit, CallExpr, Callee, Expr, Lit, MemberExpr, MemberProp};

use crate::utils::paren::strip_parens;

/// Match `Array(<n>).concat([...])` — webpack's sparse-array header when the
/// smallest module id is non-zero. Returns the array literal and the id
/// offset `n`.
pub(super) fn split_array_concat(call: &CallExpr) -> Option<(&ArrayLit, usize)> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee) else {
        return None;
    };
    let MemberProp::Ident(concat_ident) = prop else {
        return None;
    };
    if concat_ident.sym.as_ref() != "concat" {
        return None;
    }
    let Expr::Call(array_call) = strip_parens(obj) else {
        return None;
    };
    let Callee::Expr(array_callee) = &array_call.callee else {
        return None;
    };
    let Expr::Ident(array_ident) = strip_parens(array_callee) else {
        return None;
    };
    if array_ident.sym.as_ref() != "Array" {
        return None;
    }
    if array_call.args.len() != 1 || array_call.args[0].spread.is_some() {
        return None;
    }
    let id_offset = numeric_id_from_expr(&array_call.args[0].expr)?;
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Expr::Array(array) = strip_parens(&call.args[0].expr) else {
        return None;
    };
    Some((array, id_offset))
}

/// A non-negative integer literal module id. Bounded to `u32::MAX`: real
/// module ids sit far below it, and an unbounded literal
/// (`Array(1e100).concat([...])`) would saturate the float→usize cast and
/// overflow the `id_offset + index` arithmetic downstream.
pub(super) fn numeric_id_from_expr(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return None;
    }
    Some(value as usize)
}
