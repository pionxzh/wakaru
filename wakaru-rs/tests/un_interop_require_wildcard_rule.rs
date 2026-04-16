mod common;
use common::{assert_eq_normalized, render};

#[test]
fn unwraps_wildcard_by_import_path() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"));
console.log(_a);
"#;
    let expected = r#"
import * as _a from "a";
console.log(_a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_wildcard_two_args() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _b = _interopRequireWildcard(require("b"), true);
console.log(_b);
"#;
    let expected = r#"
import * as _b from "b";
console.log(_b);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_wildcard_for_non_require_args() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var ns = _interopRequireWildcard(factory());
console.log(ns.default);
"#;
    let output = render(input);
    // Non-require arg must NOT be unwrapped — helper synthesizes namespace object
    assert!(output.contains(".default"), "should preserve .default for non-require wrapped binding");
}

#[test]
fn removes_wildcard_helper_declaration() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"));
"#;
    let output = render(input);
    assert!(!output.contains("_interopRequireWildcard"), "helper declaration should be removed");
}
