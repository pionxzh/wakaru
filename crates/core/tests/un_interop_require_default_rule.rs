mod common;
use common::{assert_eq_normalized, render, render_pipeline_until};

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
fn default_rewrite_strips_only_the_interop_wrapper_layer() {
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _a = _interopRequireDefault(require("a"));
console.log(_a.default.default);
"#;
    let expected = r#"
import _a from "a";
console.log(_a.default);
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
fn unwraps_tslib_namespace_import_default_require() {
    let input = r#"
var tslib_1 = require("tslib");
var foo_1 = tslib_1.__importDefault(require("foo"));
console.log(foo_1.default);
"#;
    let expected = r#"
import tslib_1 from "tslib";
import foo_1 from "foo";
console.log(foo_1);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_tslib_direct_import_default_require() {
    let input = r#"
var foo_1 = require("tslib").__importDefault(require("foo"));
console.log(foo_1.default);
"#;
    let expected = r#"
import foo_1 from "foo";
console.log(foo_1);
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
console.log(_d);
"#;
    let expected = r#"
import _d from "d";
console.log(_d);
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
    insta::assert_snapshot!(output);
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
    insta::assert_snapshot!(output);
}

#[test]
fn skips_default_rewrite_for_reassigned_binding() {
    // _a is reassigned, so _a.default must NOT be rewritten to _a
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _a = _interopRequireDefault(require("a"));
_a = other;
console.log(_a.default);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
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

#[test]
fn does_not_unwrap_non_interop_iife_with_esmodule_guard() {
    // Regression: any IIFE starting with __esModule check was being unwrapped,
    // dropping side effects and alternate return paths
    let input = r#"
const x = ((e) => {
    if (e && e.__esModule) { return e; }
    sideEffect(e);
    return fallback;
})(input);
console.log(x);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn unwraps_inline_ternary_iife() {
    // Inline IIFE using ternary form (not if/return) — the pattern from webpack4 module-27.
    // Previously required a double-pass: UnConditionals expanded the ternary first,
    // then UnInteropRequireDefault matched the if/return form on re-parse.
    let input = r#"
var i = function(e) {
    return e && e.__esModule ? e : { default: e };
}(require("./module-36.js"));
console.log(i.default);
"#;
    let expected = r#"
import i from "./module-36.js";
console.log(i);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_swc_external_interop_require_default() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _a = _interop_require_default(require("a"));
console.log(_a.default);
"#;
    let expected = r#"
import _a from "a";
console.log(_a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_swc_assignment_form_for_same_require_binding() {
    // SWC AMD output receives dependencies as factory parameters, then wraps
    // those same bindings with a standalone interop assignment.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
console.log(_react.default.createElement("div"));
"#;
    let expected = r#"
import _react from "react";
console.log(_react.createElement("div"));
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn assignment_form_requires_the_same_target_and_argument_binding() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
var wrapped;
wrapped = _interop_require_default(_react);
console.log(wrapped.default);
"#;
    let expected = r#"
var _react = require("react");
var wrapped;
wrapped = _react;
console.log(wrapped.default);
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_must_precede_other_uses_of_the_require_binding() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
observe(_react.default);
_react = _interop_require_default(_react);
console.log(_react.default);
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
observe(_react.default);
_react = _interop_require_default(_react);
console.log(_react.default);
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_must_be_an_unconditional_top_level_statement() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
if (enabled) {
    _react = _interop_require_default(_react);
}
console.log(_react.default);
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
if (enabled) {
    _react = _interop_require_default(_react);
}
console.log(_react.default);
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_fails_closed_on_a_later_loop_head_write() {
    // A for-of head reassigns the binding without an AssignExpr; removing the
    // wrapper while rewriting `.default` would read the loop element instead
    // of its `.default` property.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
for (_react of list) {
    use2(_react.default);
}
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
for (_react of list) {
    use2(_react.default);
}
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_fails_closed_on_a_later_destructuring_write() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
[_react] = replacements;
use2(_react.default);
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
[_react] = replacements;
use2(_react.default);
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_fails_closed_on_a_deferred_write() {
    // The write lives in a function body, so it can run at any time relative
    // to the module's `.default` reads.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
function swap(next) {
    _react = next;
}
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
_react = _interop_require_default(_react);
use(_react.default);
function swap(next) {
    _react = next;
}
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn assignment_form_fails_closed_on_hoisted_pre_initializer_use() {
    // `probe()` runs before the wrapper is installed and reads `.default`
    // through a hoisted function declared after the initializer; the textual
    // statement order alone cannot prove the initializer is the first use.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
probe();
_react = _interop_require_default(_react);
use(_react.default);
function probe() {
    return _react.default;
}
"#;
    let expected = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
probe();
_react = _interop_require_default(_react);
use(_react.default);
function probe() {
    return _react.default;
}
"#;
    assert_eq_normalized(
        &render_pipeline_until(input, "UnInteropRequireDefault"),
        expected,
    );
}

#[test]
fn rejected_assignment_form_remains_a_mutable_local_after_require_recovery() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
probe();
_react = _interop_require_default(_react);
use(_react.default);
function probe() {
    return _react.default;
}
"#;
    let output = render(input);

    assert!(
        output.contains("from \"@swc/helpers/_/_interop_require_default\"")
            && output.contains("_react.default")
            && !output.contains("_react = _react"),
        "a rejected wrapper assignment must keep its runtime semantics:\n{output}"
    );
    assert!(
        output.contains("import __react from \"react\"")
            && output.contains("let _react = __react"),
        "the reassigned require local must stay mutable instead of becoming an import binding:\n{output}"
    );
}

#[test]
fn assignment_form_allows_generated_export_preamble() {
    // SWC's AMD preamble installs export getters before the interop
    // initializers: `Object.defineProperty` scaffolding and an inert local
    // `_export` helper must not block the recovery.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
Object.defineProperty(exports, "__esModule", { value: true });
function _export(target, all) {
    for (var name in all) Object.defineProperty(target, name, {
        enumerable: true,
        get: Object.getOwnPropertyDescriptor(all, name).get
    });
}
_export(exports, {
    useThing: function() { return useThing; }
});
_react = _interop_require_default(_react);
function useThing() {
    return _react.default.useEffect;
}
"#;
    let output = render_pipeline_until(input, "UnInteropRequireDefault");
    assert!(
        !output.contains("_interop_require_default(_react)")
            && !output.contains("_react = _react")
            && output.contains("_react.useEffect"),
        "the generated export preamble must not block the initializer recovery:\n{output}"
    );
}

#[test]
fn assignment_form_rejects_top_level_reflection_before_initializer() {
    // A proxy trap can transfer control into module code. Reflection is only
    // accepted inside the exact generated export helper proof, not as an
    // arbitrary top-level pre-initializer call.
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
var _react = require("react");
Object.getOwnPropertyDescriptor(proxy, "value");
_react = _interop_require_default(_react);
function probe() {
    return _react.default;
}
"#;
    let output = render_pipeline_until(input, "UnInteropRequireDefault");
    assert!(
        output.contains("_react = _interop_require_default(_react)")
            && output.contains("_react.default"),
        "top-level reflection must keep the recovery fail-closed:\n{output}"
    );
}

#[test]
fn unwraps_inline_ternary_arrow_iife() {
    // Same pattern but with arrow function syntax
    let input = r#"
var i = ((e) => e && e.__esModule ? e : { default: e })(require("./mod.js"));
console.log(i.default);
"#;
    let expected = r#"
import i from "./mod.js";
console.log(i);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_inline_wildcard_interop_iife() {
    // interopRequireWildcard: copies all properties + sets .default
    let input = r#"
const o = ((e) => {
    if (e && e.__esModule) { return e; }
    const t = {};
    if (e != null) { for (const n in e) { if (Object.prototype.hasOwnProperty.call(e, n)) { t[n] = e[n]; } } }
    t.default = e;
    return t;
})(require("./react"));
console.log(o.Component);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
