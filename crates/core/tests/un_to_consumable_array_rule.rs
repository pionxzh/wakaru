mod common;
use common::{assert_eq_normalized, render};

#[test]
fn replaces_to_consumable_array_with_spread() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a);
"#;
    let expected = r#"
const x = [...a];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_esm_import() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/esm/toConsumableArray");
var x = _toConsumableArray(arr);
"#;
    let expected = r#"
const x = [...arr];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_helper_when_untransformed_calls_remain() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a, b);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");
var x = _toConsumableArray(a);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

// ---------------------------------------------------------------------------
// Body-shape detection: inlined helper forms
// ---------------------------------------------------------------------------

#[test]
fn detects_inlined_babel6_form() {
    // Babel 6: Array.isArray + Array.from
    let input = r#"
function _toConsumableArray(arr) {
    if (Array.isArray(arr)) {
        for (var i = 0, arr2 = Array(arr.length); i < arr.length; i++) arr2[i] = arr[i];
        return arr2;
    } else {
        return Array.from(arr);
    }
}
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inlined_babel7_form() {
    // Babel 7+: logical-OR chain of sub-helper calls.
    // The module must also contain a sub-helper with Array.isArray/Array.from
    // for the OR-chain to be accepted (prevents false positives).
    // DeadDecls removes _arrayWithoutHoles since its only caller was eliminated.
    let input = r#"
function _arrayWithoutHoles(arr) {
    if (Array.isArray(arr)) return _arrayLikeToArray(arr);
}
function _toConsumableArray(arr) {
    return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _unsupportedIterableToArray(arr) || _nonIterableSpread();
}
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_minified_to_consumable_array() {
    // Minified: short name, same structure
    let input = r#"
function a(e) {
    if (Array.isArray(e)) {
        for (var t = 0, n = new Array(e.length); t < e.length; t++) n[t] = e[t];
        return n;
    } else {
        return Array.from(e);
    }
}
var x = a(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_var_assigned_to_consumable_array() {
    // DeadDecls removes _arrayWithoutHoles since its only caller was eliminated.
    let input = r#"
function _arrayWithoutHoles(arr) {
    if (Array.isArray(arr)) return _arrayLikeToArray(arr);
}
var _toConsumableArray = function(arr) {
    return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _unsupportedIterableToArray(arr) || _nonIterableSpread();
};
var x = _toConsumableArray(items);
"#;
    let expected = r#"
const x = [...items];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_false_positive_single_param_unrelated() {
    // A single-param function that doesn't match the helper shape
    let input = r#"
function transform(arr) {
    return arr.map(function(x) { return x + 1; });
}
var x = transform(items);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_false_positive_or_chain_fallback() {
    // A normal fallback pipeline that happens to be a 1-arg OR chain
    let input = r#"
function choose(arr) {
    return parse(arr) || normalize(arr) || fallback(arr) || die();
}
var x = choose(items);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
