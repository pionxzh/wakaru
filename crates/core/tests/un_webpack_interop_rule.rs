mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::UnWebpackInterop;

/// Render applying only UnWebpackInterop in isolation.
fn render(input: &str) -> String {
    render_rule(input, UnWebpackInterop::new)
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
    insta::assert_snapshot!(output);
}

#[test]
fn getter_used_as_value_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
var ref = _lib2;
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn getter_member_not_dot_a_not_inlined() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
console.log(_lib2.b);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
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
    insta::assert_snapshot!(output);
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
    insta::assert_snapshot!(output);
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

// ── Direct webpack require.n helper form ───────────────────────────

#[test]
fn direct_require_n_getter_call_inlined_for_require_binding() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = require.n(_lib);
console.log(_lib2().method());
"#;
    let expected = r#"
var _lib = require("./lib");
console.log(_lib.method());
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn direct_require_n_getter_call_inlined_for_import_binding() {
    let input = r#"
import _lib from "./lib";
const _lib2 = require.n(_lib);
console.log(_lib2()().astSync(source));
"#;
    let expected = r#"
import _lib from "./lib";
console.log(_lib().astSync(source));
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn direct_require_n_getter_dot_a_inlined() {
    let input = r#"
import _lib from "./lib";
const _lib2 = require.n(_lib);
console.log(_lib2.a);
"#;
    let expected = r#"
import _lib from "./lib";
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn direct_require_n_getter_used_as_value_not_inlined() {
    let input = r#"
import _lib from "./lib";
const _lib2 = require.n(_lib);
const ref = _lib2;
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn pipeline_inlines_direct_require_n_after_esm_imports_exist() {
    let input = r#"
import _lib from "./lib";
const _lib2 = require.n(_lib);
console.log(_lib2()().astSync(source));
"#;
    let expected = r#"
import _lib from "./lib";
console.log(_lib().astSync(source));
"#;
    assert_eq_normalized(&render_pipeline(input), expected.trim());
}

// ── Webpack require.t namespace helper form ────────────────────────

#[test]
fn require_t_mode_2_named_property_read_uses_module_binding() {
    let input = r#"
const react = require("./react");
let useId = require.t(react, 2).useId || (() => undefined);
"#;
    let expected = r#"
const react = require("./react");
let useId = react.useId || (() => undefined);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_t_mode_2_cached_namespace_property_read_uses_module_binding() {
    let input = r#"
let ns;
const react = require("./react");
let useId = (ns || (ns = require.t(react, 2)))["useId".toString()] || (() => undefined);
"#;
    let expected = r#"
const react = require("./react");
let useId = react["useId".toString()] || (() => undefined);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_t_cached_namespace_with_later_cache_use_not_inlined() {
    let input = r#"
let ns;
const react = require("./react");
let useId = (ns || (ns = require.t(react, 2))).useId;
console.log(ns);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn require_t_mode_2_default_property_read_returns_module_binding() {
    let input = r#"
const lib = require("./lib");
let value = require.t(lib, 2).default;
"#;
    let expected = r#"
const lib = require("./lib");
let value = lib;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_t_dynamic_property_read_not_inlined() {
    let input = r#"
const react = require("./react");
let useId = require.t(react, 2)[name];
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn require_t_mode_2_of_namespace_import_is_that_namespace() {
    // The namespace import already is the object `require.t` builds for the
    // CommonJS module it imported.
    let input = r#"
import * as dep from "./dep.js";
let ns;
use(ns || (ns = require.t(dep, 2)));
consume(require.t(dep, 2));
"#;
    let expected = r#"
import * as dep from "./dep.js";
use(dep);
consume(dep);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_t_mode_2_whole_value_of_default_import_not_inlined() {
    // A default import of a CommonJS module is `module.exports`, not the
    // namespace object `require.t` builds from it.
    let input = r#"
import dep from "./dep.js";
use(require.t(dep, 2));
"#;
    let output = render(input);
    assert!(output.contains("use(require.t(dep, 2))"), "{output}");
}

#[test]
fn require_t_non_mode_2_not_inlined() {
    let input = r#"
const lib = require("./lib");
let value = require.t(lib, 19).default;
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

// ── Webpack require.o hasOwnProperty helper form ───────────────────

#[test]
fn require_o_rewrites_to_has_own_property_call() {
    let input = r#"
if (!require.o(map, request)) {
  throw new Error("missing");
}
"#;
    let expected = r#"
if (!Object.prototype.hasOwnProperty.call(map, request)) {
  throw new Error("missing");
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_o_with_spread_arg_not_rewritten() {
    let input = r#"
const ok = require.o(map, ...keys);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn shadowed_require_o_not_rewritten() {
    let input = r#"
function outer(require) {
  return require.o(map, request);
}
"#;
    assert_eq_normalized(&render(input), input);
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
    insta::assert_snapshot!(output);
}

#[test]
fn shadowed_require_binding_not_matched() {
    let input = r#"
function outer(require) {
  var _lib = require("./lib");
  var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
  console.log(_lib2());
}
"#;
    assert_eq_normalized(&render(input), input);
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

#[test]
fn getter_replacement_avoids_catch_param_shadowing() {
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
function compile() {
  try {
    return null;
  } catch (r) {
    return o();
  }
}
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
function compile() {
  try {
    return null;
  } catch (r) {
    return _r;
  }
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn getter_replacement_avoids_block_scope_shadowing() {
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
{
  let r = {};
  console.log(o());
}
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
{
  let r = {};
  console.log(_r);
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Computed object keys ───────────────────────────────────────────

#[test]
fn getter_call_in_computed_object_key_is_inlined() {
    let input = r#"
var l = require("./styles.js"), c = () => l && l.__esModule ? l.default : l;
exports.A = { [c().active]: true, x: c().base };
"#;
    let expected = r#"
var l = require("./styles.js");
exports.A = {
    [l.active]: true,
    x: l.base
};
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn getter_replacement_avoids_shadowing_from_a_computed_key_reference() {
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
function compile(pattern) {
  var r = {};
  return { [o()(pattern)]: r };
}
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
function compile(pattern) {
  var r = {};
  return {
    [_r(pattern)]: r
  };
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn require_t_cached_namespace_used_in_computed_key_not_inlined() {
    let input = r#"
let ns;
const react = require("./react");
let useId = (ns || (ns = require.t(react, 2))).useId;
const keys = { [ns.version]: 1 };
"#;
    let output = render(input);
    assert!(output.contains("let ns;"), "{output}");
    assert!(output.contains("ns = require.t(react, 2)"), "{output}");
    assert!(output.contains("[ns.version]: 1"), "{output}");
}

#[test]
fn base_rename_reaches_getters_whose_uses_are_not_shadowed() {
    // One getter is used where a local `r` shadows the base, which forces the
    // base to be renamed module-wide. The other getter's replacement must
    // follow that rename, or it points at a name that no longer exists.
    let input = r#"
var r = require("./path-to-regexp");
var o = () => r && r.__esModule ? r.default : r;
var i = () => r && r.__esModule ? r.default : r;
function compile(pattern, options) {
  var r = {};
  return o()(pattern, [], options);
}
var parse = i().parse;
"#;
    let expected = r#"
var _r = require("./path-to-regexp");
function compile(pattern, options) {
  var r = {};
  return _r(pattern, [], options);
}
var parse = _r.parse;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Dynamic scope ──────────────────────────────────────────────────

#[test]
fn inlines_getter_but_keeps_has_own_in_module_with_direct_eval() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
eval(s);
console.log(_lib2(), require.o(a, b));
"#;
    let expected = r#"
var _lib = require("./lib");
eval(s);
console.log(_lib, require.o(a, b));
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn inlines_getter_used_inside_function_with_direct_eval() {
    let input = r#"
import _lib from "./lib";
var _lib2 = () => { if (_lib && _lib.__esModule) { return _lib.default; } return _lib; };
let colors = _lib2();
async function parse(source) {
  await eval(source);
  return _lib2.a;
}
"#;
    let expected = r#"
import _lib from "./lib";
let colors = _lib;
async function parse(source) {
  await eval(source);
  return _lib;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn inlines_require_n_getter_in_module_with_direct_eval() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = require.n(_lib);
eval(s);
console.log(_lib2());
"#;
    let expected = r#"
var _lib = require("./lib");
eval(s);
console.log(_lib);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn keeps_getter_that_needs_a_base_rename_in_module_with_direct_eval() {
    let input = r#"
var r = require("./lib");
var o = () => r && r.__esModule ? r.default : r;
function compile(pattern) {
  var r = {};
  return o()(pattern, r);
}
eval(s);
"#;
    assert_eq_normalized(&render(input), input.trim());
}

#[test]
fn inlines_cached_namespace_in_module_with_direct_eval() {
    let input = r#"
let ns;
const react = require("./react");
let useId = (ns || (ns = require.t(react, 2))).useId;
async function load(source) {
  await eval(source);
  return require.t(react, 2).default;
}
"#;
    let expected = r#"
const react = require("./react");
let useId = react.useId;
async function load(source) {
  await eval(source);
  return react;
}
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn keeps_getter_and_has_own_in_module_with_with_statement() {
    let input = r#"
var _lib = require("./lib");
var _lib2 = () => _lib && _lib.__esModule ? _lib.default : _lib;
with (o) {
    console.log(_lib2(), require.o(a, b));
}
"#;
    assert_eq_normalized(&render(input), input.trim());
}
