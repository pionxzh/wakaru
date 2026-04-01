mod common;
use common::{assert_eq_normalized, render};

#[test]
fn replaces_object_spread2_with_spread_syntax() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_multiple_spread_sources() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, a, b, c);
"#;
    let expected = r#"
const x = { ...a, ...b, ...c };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_extends_helper() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends({}, obj1, obj2);
"#;
    let expected = r#"
const x = { ...obj1, ...obj2 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_nested_babel_pattern() {
    // Babel generates nested _objectSpread2 calls, each with {} as first arg:
    // _objectSpread2(_objectSpread2({}, a), {}, { b: 1 })
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, a, { b: 1 });
"#;
    let expected = r#"
const x = { ...a, b: 1 };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_esm_object_spread() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/esm/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_fallback_object_spread() {
    let input = r#"
var _objectSpread = require("@babel/runtime/helpers/objectSpread");
var x = _objectSpread({}, y);
"#;
    let expected = r#"
const x = { ...y };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_non_empty_first_arg() {
    // Both _extends and _objectSpread2 mutate their first arg.
    // Non-empty first arg must be preserved.
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2(target, { a: 1 });
"#;
    let output = render(input);
    assert!(output.contains("_objectSpread2"), "should not transform with real target");
}

#[test]
fn extends_preserves_non_empty_target() {
    let input = r#"
var _extends = require("@babel/runtime/helpers/extends");
var x = _extends(target, source);
"#;
    let output = render(input);
    assert!(output.contains("_extends"), "should not transform _extends with real target");
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var x = _objectSpread2({}, y);
"#;
    let output = render(input);
    assert!(!output.contains("_objectSpread2"), "helper should be removed");
}
