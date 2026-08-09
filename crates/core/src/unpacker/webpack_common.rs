//! Container syntax shared by webpack 4 and webpack 5 output.
//!
//! Both major versions render module tables through the same template
//! (`Template.getModulesArrayBounds`): a sparse array when module ids are
//! dense numerics, wrapped in `Array(minId).concat([...])` when the smallest
//! id is non-zero. The matchers live here so neither version's unpacker owns
//! the other's syntax.

use std::collections::HashSet;

use swc_core::ecma::ast::{ArrayLit, CallExpr, Callee, Expr, Lit, MemberExpr, MemberProp};

use super::emit_esm::{dedup_filename, FilenameDedupStyle};
use crate::utils::paren::strip_parens;

const JAVASCRIPT_LIKE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"];

/// Derive a truthful JavaScript output filename from a webpack module id.
///
/// String ids preserve their sanitized resource path. Loader queries and URL
/// fragments are identities inside webpack's table, not filesystem path
/// components, so they are removed here; [`unique_webpack_module_filenames`]
/// then keeps multiple virtual modules for the same resource distinct.
/// Non-JavaScript source extensions are retained as provenance and followed by
/// `.js` (`style.less` -> `style.less.js`). Numeric ids keep webpack's
/// established `module-<id>.js` naming discipline.
pub(super) fn webpack_module_filename(module_id: &str) -> String {
    if module_id.parse::<i64>().is_ok() {
        return format!("module-{module_id}.js");
    }

    let resource_end = module_id
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '?' | '#').then_some(index))
        .unwrap_or(module_id.len());
    let resource = &module_id[..resource_end];
    if resource.is_empty() {
        return "unknown.js".to_string();
    }
    let path_like = resource.contains(['/', '\\', '.']) || resource_end != module_id.len();
    if !path_like {
        return format!("module-{resource}.js");
    }

    let mut filename = super::sanitize_relative_path(resource, "unknown.js");
    let javascript_like = std::path::Path::new(&filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            JAVASCRIPT_LIKE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        });
    if !javascript_like {
        filename.push_str(".js");
    }
    filename
}

/// Allocate webpack module filenames in table order before synthesizing any
/// consumer edge. Doing this at the id->filename boundary is required for
/// collisions such as `a.less` / `a.less.js` and queried virtual modules: a
/// later old-filename rewrite cannot recover which original id owned an edge
/// once two provisional filenames are identical.
pub(super) fn unique_webpack_module_filenames<'a>(
    module_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    module_ids
        .into_iter()
        .map(|module_id| {
            let filename = webpack_module_filename(module_id);
            dedup_filename(
                &filename,
                &mut seen,
                FilenameDedupStyle::PathAware {
                    fallback_stem: "module",
                },
            )
        })
        .collect()
}

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

/// A non-negative integer literal module id. Bounded to `i32::MAX`: real
/// module ids sit far below it, and an unbounded literal
/// (`Array(1e100).concat([...])`) would saturate the float→usize cast and
/// overflow the `id_offset + index` arithmetic downstream — including on
/// wasm32, where `usize` is 32 bits and even `u32::MAX + index` wraps.
pub(super) fn numeric_id_from_expr(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 || value > f64::from(i32::MAX) {
        return None;
    }
    Some(value as usize)
}
