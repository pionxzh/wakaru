mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnWebpackInterop;

/// Render applying only UnWebpackInterop in isolation.
fn render(input: &str) -> String {
    render_rule(input, |_| UnWebpackInterop)
}

// ── Ternary (expression) form ──────────────────────────────────────

#[test]
fn ternary_getter_call_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2());
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn ternary_getter_dot_a_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2.a);
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Block-statement (if/return) form ───────────────────────────────

#[test]
fn block_getter_call_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => { if (_lib && _lib.__esModule) { return _lib.default; } return _lib; };
console.log(_lib2());
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn block_getter_dot_a_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => { if (_lib && _lib.__esModule) { return _lib.default; } return _lib; };
console.log(_lib2.a);
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Multiple getters ───────────────────────────────────────────────

#[test]
fn multiple_getters_all_inlined() {
    let input = r#"
var _a = require("./a");
var _b = require("./b");
var _a2 = () => _a && _a.__esModule ? _a.default : _a;
var _b2 = () => _b && _b.__esModule ? _b.default : _b;
console.log(_a2(), _b2());
"#;
    let expected = r#"
var _a = require("./a");
var _b = require("./b");
console.log(_a, _b);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Unsafe usages → getter is kept ─────────────────────────────────

#[test]
fn getter_called_with_args_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2("unexpected"));
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept when called with args"
    );
}

#[test]
fn getter_used_as_value_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
var ref = _lib2;
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept when used as a value"
    );
}

#[test]
fn getter_member_not_dot_a_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2.b);
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept when accessed with .b (not .a)"
    );
}

// ── Non-require base → no match ────────────────────────────────────

#[test]
fn non_require_base_not_matched() {
    let input = r#"
var _lib = someOtherCall();
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2());
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept when base is not a require() call"
    );
}

// ── Mixed safe and unsafe usage → getter is kept ───────────────────

#[test]
fn mixed_usage_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2());
var ref = _lib2;
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept with mixed safe/unsafe usage"
    );
}

// ── Computed property access forms ─────────────────────────────────

#[test]
fn computed_esmodule_access() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib["__esModule"] ? _lib["default"] : _lib;
console.log(_lib2());
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn computed_dot_a_access() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2["a"]);
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── No require bindings → rule is a no-op ──────────────────────────

#[test]
fn no_require_bindings_noop() {
    let input = r#"
var _lib = import("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2());
"#;
    let output = render(input);
    assert!(
        output.contains("_lib2"),
        "getter should be kept when no require() bindings exist"
    );
}

#[test]
fn getter_replacement_avoids_inner_shadowing() {
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
var holder = { r };
function compile(pattern, options) {
  var r = {};
  return [holder, o()(pattern, [], options)];
}
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
var holder = {
  r: _r
};
function compile(pattern, options) {
  var r = {};
  return [holder, _r(pattern, [], options)];
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn getter_replacement_avoids_later_scope_shadowing() {
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
function compile(pattern, options) {
  const result = o()(pattern, [], options);
  var r = {};
  return result;
}
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
function compile(pattern, options) {
  const result = _r(pattern, [], options);
  var r = {};
  return result;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}
