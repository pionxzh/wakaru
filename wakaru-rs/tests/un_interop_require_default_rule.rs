mod common;
use common::{assert_eq_normalized, render};

#[test]
fn unwraps_interop_require_default_by_import_path() {
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _a = _interopRequireDefault(require("a"));
console.log(_a.default);
"#;
    let expected = r#"
import _a from "a";
console.log(_a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_interop_require_default_by_esm_import_path() {
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/esm/interopRequireDefault");
var _b = _interopRequireDefault(require("b"));
_b.default();
"#;
    let expected = r#"
import _b from "b";
_b();
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inlined_ternary_form() {
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _a = _interopRequireDefault(require("a"));
console.log(_a.default);
"#;
    let expected = r#"
import _a from "a";
console.log(_a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_inlined_if_return_form() {
    let input = r#"
function _interopRequireDefault(obj) {
    if (obj && obj.__esModule) { return obj; }
    return { default: obj };
}
var _a = _interopRequireDefault(require("a"));
_a.default();
"#;
    let expected = r#"
import _a from "a";
_a();
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn detects_minified_names() {
    let input = r#"
function a(b) {
    return b && b.__esModule ? b : { default: b };
}
var _c = a(require("c"));
console.log(_c.default);
"#;
    let expected = r#"
import _c from "c";
console.log(_c);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_var_assigned_function_expression() {
    let input = r#"
var _interopRequireDefault = function(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
};
var _a = _interopRequireDefault(require("a"));
_a.default;
"#;
    let expected = r#"
import _a from "a";
_a;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_direct_dot_default() {
    // interopRequireDefault(require("x")).default → require("x")
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _d = _interopRequireDefault(require("d")).default;
"#;
    let expected = r#"
import _d from "d";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn removes_helper_declaration() {
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _a = _interopRequireDefault(require("a"));
"#;
    let output = render(input);
    assert!(!output.contains("_interopRequireDefault"), "helper declaration should be removed");
    assert!(!output.contains("__esModule"), "__esModule check should be removed");
}

#[test]
fn no_false_positive_on_non_matching_function() {
    let input = r#"
function notAHelper(obj) {
    return obj.foo;
}
var _a = notAHelper(require("a"));
"#;
    let output = render(input);
    // Should still contain the function — not detected as helper
    assert!(output.contains("notAHelper"), "should not remove non-helper function");
}

#[test]
fn skips_default_rewrite_for_reassigned_binding() {
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _a = _interopRequireDefault(require("a"));
_a = other;
console.log(_a.default);
"#;
    let output = render(input);
    // _a is reassigned, so _a.default must NOT be rewritten to _a
    assert!(output.contains(".default"), "should preserve .default for reassigned binding");
}

#[test]
fn handles_require_default_import_path() {
    // var _ird = require("@babel/runtime/helpers/interopRequireDefault").default
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault").default;
var _a = _interopRequireDefault(require("a"));
_a.default;
"#;
    let expected = r#"
import _a from "a";
_a;
"#;
    assert_eq_normalized(&render(input), expected);
}
