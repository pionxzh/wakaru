mod common;
use common::{assert_eq_normalized, render};

#[test]
fn unwraps_sliced_to_array() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 2);
var name = _ref[0];
var value = _ref[1];
"#;
    // slicedToArray just unwraps; destructuring reconstruction is done by downstream rules
    let output = render(input);
    assert!(!output.contains("_slicedToArray"), "helper call should be unwrapped");
    assert!(!output.contains("slicedToArray"), "helper declaration should be removed");
}

#[test]
fn handles_zero_length() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 0);
"#;
    let expected = r#"
var [] = a;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_esm_import() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/esm/slicedToArray");
var _ref = _slicedToArray(expr, 3);
var x = _ref[0];
"#;
    let output = render(input);
    assert!(!output.contains("_slicedToArray"), "helper should be unwrapped");
}

#[test]
fn skips_invalid_arg_counts() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
_slicedToArray();
_slicedToArray(a);
_slicedToArray(a, 2, 3);
"#;
    let output = render(input);
    // Invalid calls should not be transformed, helper should remain
    assert!(output.contains("_slicedToArray"), "should not transform invalid calls");
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _ref = _slicedToArray(a, 2);
var name = _ref[0];
"#;
    let output = render(input);
    assert!(!output.contains("require(\"@babel/runtime"), "helper import should be removed");
}

// ---------------------------------------------------------------------------
// Body-shape detection: inlined helper forms
// ---------------------------------------------------------------------------

#[test]
fn detects_inlined_babel6_sliced_to_array() {
    // Babel 6: references Symbol.iterator
    let input = r#"
function _slicedToArray(arr, i) {
    if (Array.isArray(arr)) {
        return arr;
    } else if (Symbol.iterator in Object(arr)) {
        var _arr = [];
        var _n = true;
        var _d = false;
        var _e = undefined;
        try {
            for (var _i = arr[Symbol.iterator](), _s; !(_n = (_s = _i.next()).done); _n = true) {
                _arr.push(_s.value);
                if (i && _arr.length === i) break;
            }
        } catch (err) { _d = true; _e = err; }
        finally { try { if (!_n && _i["return"]) _i["return"](); } finally { if (_d) throw _e; } }
        return _arr;
    } else {
        throw new TypeError("Invalid attempt to destructure non-iterable instance");
    }
}
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let output = render(input);
    assert!(!output.contains("_slicedToArray"), "helper should be unwrapped");
}

#[test]
fn detects_inlined_babel7_sliced_to_array() {
    // Babel 7+: logical-OR chain of sub-helper calls.
    // Module must also contain a sub-helper with Array.isArray for corroboration.
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
function _slicedToArray(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
}
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
var value = _ref[1];
"#;
    let output = render(input);
    assert!(!output.contains("_slicedToArray"), "helper should be unwrapped");
}

#[test]
fn detects_minified_sliced_to_array() {
    let input = r#"
function c(e) {
    if (Array.isArray(e)) return e;
}
function r(e, t) {
    return c(e) || o(e, t) || s(e, t) || l();
}
var _ref = r(pair, 2);
var key = _ref[0];
"#;
    let output = render(input);
    assert!(!output.contains("function r"), "minified helper should be detected and removed");
}

#[test]
fn detects_var_assigned_sliced_to_array() {
    let input = r#"
function _arrayWithHoles(arr) {
    if (Array.isArray(arr)) return arr;
}
var _slicedToArray = function(arr, i) {
    return _arrayWithHoles(arr) || _iterableToArrayLimit(arr, i) || _unsupportedIterableToArray(arr, i) || _nonIterableRest();
};
var _ref = _slicedToArray(pair, 2);
var key = _ref[0];
"#;
    let output = render(input);
    assert!(!output.contains("_slicedToArray"), "var-assigned helper should be detected");
}

#[test]
fn no_false_positive_two_param_unrelated() {
    // A two-param function that doesn't match the helper shape
    let input = r#"
function slice(arr, count) {
    return arr.slice(0, count);
}
var x = slice(items, 3);
"#;
    let output = render(input);
    assert!(output.contains("slice"), "should not detect unrelated function as helper");
}

#[test]
fn no_false_positive_symbol_iterator_utility() {
    // A 2-param function that references Symbol.iterator but isn't slicedToArray
    let input = r#"
function maybeIter(arr, count) {
    if (Symbol.iterator in Object(arr)) return take(arr, count);
    return arr.slice(0, count);
}
var x = maybeIter(items, 2);
"#;
    let output = render(input);
    assert!(output.contains("maybeIter"), "should not detect iterator utility as slicedToArray");
}

#[test]
fn no_false_positive_or_chain_two_params() {
    // A 2-param OR chain that isn't slicedToArray
    let input = r#"
function resolve(a, b) {
    return tryFirst(a) || trySecond(a, b) || tryThird(a, b) || giveUp();
}
var x = resolve(items, 2);
"#;
    let output = render(input);
    assert!(output.contains("resolve"), "should not detect normal OR-chain as slicedToArray");
}
