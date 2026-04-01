mod common;
use common::{assert_eq_normalized, render};

#[test]
fn unwraps_wildcard_by_import_path() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _a = _interopRequireWildcard(require("a"));
"#;
    let expected = r#"
import * as _a from "a";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_wildcard_two_args() {
    let input = r#"
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _b = _interopRequireWildcard(require("b"), true);
"#;
    let expected = r#"
import * as _b from "b";
"#;
    assert_eq_normalized(&render(input), expected);
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
