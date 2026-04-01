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
