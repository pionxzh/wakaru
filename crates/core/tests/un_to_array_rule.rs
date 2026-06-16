mod common;
use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::rules::UnToArray;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnToArray)
}

#[test]
fn unwraps_swc_external_to_array_destructuring() {
    // Isolated: the array-rest pattern already exists (UnDestructuring builds it
    // in the full pipeline); UnToArray strips the helper and drops the import.
    let input = r#"
import { _ as _to_array } from "@swc/helpers/_/_to_array";
var [first, ...rest_items] = _to_array(items);
use(first, rest_items);
"#;
    let expected = r#"
var [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unwraps_unaliased_swc_to_array() {
    let input = r#"
import { _ } from "@swc/helpers/_/_to_array";
var [first, ...rest_items] = _(items);
use(first, rest_items);
"#;
    let expected = r#"
var [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unwraps_babel_runtime_to_array_default_import() {
    let input = r#"
import _toArray from "@babel/runtime/helpers/toArray";
var [first, ...rest_items] = _toArray(items);
use(first, rest_items);
"#;
    let expected = r#"
var [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unwraps_babel_runtime_to_array_require() {
    let input = r#"
var _toArray = require("@babel/runtime/helpers/toArray");
var [first, ...rest_items] = _toArray(items);
use(first, rest_items);
"#;
    let expected = r#"
var [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn unwraps_array_destructuring_assignment_target() {
    let input = r#"
import { _ as _to_array } from "@swc/helpers/_/_to_array";
[first, ...rest_items] = _to_array(items);
use(first, rest_items);
"#;
    let expected = r#"
[first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_helper_when_not_array_destructuring() {
    // No array pattern -> the toArray wrapper is load-bearing (the binding may
    // escape as a real array), so it must stay, and the import with it.
    let input = r#"
import { _ as _to_array } from "@swc/helpers/_/_to_array";
var copy = _to_array(items);
use(copy);
"#;
    let output = apply(input);
    assert!(
        output.contains("_to_array(items)") || output.contains("_(items)"),
        "non-destructuring toArray call must be preserved: {output}"
    );
    assert!(
        output.contains("@swc/helpers/_/_to_array"),
        "import must be preserved while a call site remains: {output}"
    );
}

#[test]
fn preserves_non_helper_to_array_lookalike() {
    let input = r#"
var customToArray = makeToArray();
var [first, ...rest_items] = customToArray(items);
use(first, rest_items);
"#;
    let output = apply(input);
    assert!(
        output.contains("customToArray(items)"),
        "non-helper toArray-like call must be preserved: {output}"
    );
}

#[test]
fn folds_swc_external_to_array_end_to_end() {
    // Full pipeline: swc externalHelpers lowers `const [first, ...rest] = items`
    // to a temp + index/slice accesses over the helper. The whole chain should
    // recover, and the @swc/helpers import should be gone.
    let input = r#"
import { _ as _to_array } from "@swc/helpers/_/_to_array";
var _items = _to_array(items), first = _items[0], rest_items = _items.slice(1);
use(first, rest_items);
"#;
    let expected = r#"
const [first, ...rest_items] = items;
use(first, rest_items);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_maybe_array_like_to_array_on_rest_pattern() {
    let input = r#"
function _maybeArrayLike(orElse, arr, i) {
  if (arr && !Array.isArray(arr) && typeof arr.length === "number") {
    return arr;
  }
  return orElse(arr, i);
}
var [first, ...rest] = _maybeArrayLike(_toArray, items);
use(first, rest);
"#;
    let expected = r#"
var [first, ...rest] = items;
use(first, rest);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn preserves_maybe_array_like_on_non_rest_pattern() {
    let input = r#"
var [a, b] = _maybeArrayLike(_slicedToArray, pair, 2);
use(a, b);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_local_maybe_array_like_parameter() {
    let input = r#"
function f(_maybeArrayLike, _toArray, items) {
  var [first, ...rest] = _maybeArrayLike(_toArray, items);
  use(first, rest);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn preserves_unrelated_unused_function_when_maybe_array_like_is_present() {
    let input = r#"
function unrelated() {}
function _maybeArrayLike(orElse, arr, i) {
  if (arr && !Array.isArray(arr) && typeof arr.length === "number") {
    return arr;
  }
  return orElse(arr, i);
}
var [first, ...rest] = _maybeArrayLike(_toArray, items);
use(first, rest);
"#;
    let expected = r#"
function unrelated() {}
var [first, ...rest] = items;
use(first, rest);
"#;
    assert_eq_normalized(&apply(input), expected);
}
