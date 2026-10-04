mod common;
use common::{assert_eq_normalized, render, render_pipeline_until, render_rule, render_with_level};
use wakaru_core::RewriteLevel;

// The registered rule only unwraps helper runtime requires; `UnEsm` runs
// the full unwrap when it converts the module. These tests exercise the
// full unwrap and its gates directly.
fn apply_rule(input: &str) -> String {
    render_rule(input, |_| wakaru_core::rules::UnInteropRequireDefault)
}

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
fn unwraps_swc_namespace_member_assignment_form() {
    // AMD extraction preserves the helper and dependency factory parameters
    // as mutable bindings, then the generated wrapper initializes the latter.
    let input = r#"
var _interop_require_default = require("@swc/helpers/_/_interop_require_default");
var _react = require("react");
_react = _interop_require_default._(_react);
console.log(_react.default.createElement("div"));
"#;
    let expected = r#"
import _react from "react";
console.log(_react.createElement("div"));
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn swc_namespace_member_preserves_authored_const_assignment_semantics() {
    let input = r#"
const _interop_require_default = require("@swc/helpers/_/_interop_require_default");
const _react = require("react");
_react = _interop_require_default._(_react);
console.log(_react.default.createElement("div"));
"#;
    let output = render(input);

    assert!(
        output.contains("const _react") && output.contains("_react ="),
        "an authored const assignment must remain and still throw:\n{output}"
    );
    assert!(
        output.contains("_interop_require_default._(_react)") && output.contains("_react.default"),
        "the member-form wrapper must fail closed with its default layer intact:\n{output}"
    );
}

#[test]
fn unwraps_swc_namespace_member_declarator_form() {
    // Real SWC CommonJS external-helper output initializes the wrapped value
    // directly instead of using AMD's later self-assignment.
    let input = r#"
const _interop_require_default = require("@swc/helpers/_/_interop_require_default");
const _something = _interop_require_default._(require("lodash/dist/something.js"));
_something.default();
"#;
    let expected = r#"
import _something from "lodash/dist/something.js";
_something();
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn unwraps_stable_var_swc_namespace_member_declarator_form() {
    // SWC also emits `var` helper namespaces for older targets. The exact
    // runtime path plus a no-write proof makes this namespace equally stable.
    let input = r#"
var _interop_require_default = require("@swc/helpers/_/_interop_require_default");
var _something = _interop_require_default._(require("lodash/dist/something.js"));
_something.default();
"#;
    let expected = r#"
import _something from "lodash/dist/something.js";
_something();
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn reassigned_var_swc_namespace_member_fails_closed() {
    let input = r#"
var helper = require("@swc/helpers/_/_interop_require_default");
var value = require("value");
helper = decorate(helper);
value = helper._(value);
use(value.default);
"#;
    let output = render(input);

    assert!(
        output.contains("helper = decorate(helper)")
            && output.contains("value = helper._(value)")
            && output.contains("value.default"),
        "a mutable helper namespace must not inherit SWC helper semantics:\n{output}"
    );
    assert!(
        output.contains("import * as") && output.contains("helper = _"),
        "the exact helper require must remain executable through a mutable namespace alias:\n{output}"
    );
}

#[test]
fn swc_namespace_member_requires_the_exact_helper_path() {
    let input = r#"
const helper_namespace = require("user-helper");
const value = require("value");
value = helper_namespace._(value);
console.log(value.default);
"#;
    let expected = r#"
const helper_namespace = require("user-helper");
const value = require("value");
value = helper_namespace._(value);
console.log(value.default);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn swc_namespace_member_requires_the_interop_default_helper_kind() {
    // A same-binding `value = helper._(value)` is not enough: every modern
    // SWC helper module exports its callable as `_`.
    let input = r#"
const _get_prototype_of = require("@swc/helpers/_/_get_prototype_of");
const value = require("value");
value = _get_prototype_of._(value);
console.log(value.default);
"#;
    let expected = r#"
const _get_prototype_of = require("@swc/helpers/_/_get_prototype_of");
const value = require("value");
value = _get_prototype_of._(value);
console.log(value.default);
"#;
    assert_eq_normalized(&apply_rule(input), expected);
}

#[test]
fn rejected_swc_namespace_member_assignment_keeps_a_namespace_import() {
    let input = r#"
var _interop_require_default = require("@swc/helpers/_/_interop_require_default");
var _react = require("react");
probe();
_react = _interop_require_default._(_react);
use(_react.default);
function probe() {
    return _react.default;
}
"#;
    let output = render(input);

    assert!(
        output.contains(
            "import * as _interop_require_default from \"@swc/helpers/_/_interop_require_default\""
        ) && output.contains("_react = _interop_require_default._(_react)")
            && output.contains("_react.default"),
        "a rejected member-form wrapper must preserve the SWC namespace call:\n{output}"
    );
    assert!(
        output.contains("import __react from \"react\"") && output.contains("let _react = __react"),
        "the wrapped dependency must remain a mutable local:\n{output}"
    );
}

#[test]
fn referenced_swc_helper_namespace_is_not_removed_after_unwrap() {
    let input = r#"
const helper = require("@swc/helpers/_/_interop_require_default");
const value = helper._(require("value"));
observe(helper);
console.log(value.default);
"#;
    let expected = r#"
import * as helper from "@swc/helpers/_/_interop_require_default";
import value from "value";
observe(helper);
console.log(value);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn swc_namespace_member_matching_uses_binding_identity() {
    let input = r#"
const helper = require("@swc/helpers/_/_interop_require_default");
function run(helper, value) {
    return helper._(value);
}
console.log(run);
"#;
    let output = apply_rule(input);
    assert!(
        output.contains("return helper._(value)") && !output.contains("return value"),
        "a shadowing parameter must not inherit the outer helper provenance:\n{output}"
    );
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
    assert_eq_normalized(&apply_rule(input), expected);
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
fn rejected_assignment_form_preserves_authored_const_semantics() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
const react = require("react");
probe();
react = _interop_require_default(react);
"#;
    let output = render(input);

    assert!(
        output.contains("const react") && !output.contains("let react"),
        "an authored const assignment must still throw instead of being widened:\n{output}"
    );
    assert!(
        output.contains("react =")
            && output.contains("from \"@swc/helpers/_/_interop_require_default\"")
            && !output.contains("react = react"),
        "the rejected wrapper assignment must remain executable:\n{output}"
    );
}

#[test]
fn accepted_assignment_form_preserves_authored_const_semantics() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
const react = require("react");
react = _interop_require_default(react);
use(react.default);
"#;
    let output = render(input);

    assert!(
        output.contains("const react") && output.contains("react ="),
        "an authored const assignment must remain and still throw: {output}"
    );
    assert!(
        output.contains("react.default")
            && output.contains("from \"@swc/helpers/_/_interop_require_default\""),
        "the authored wrapper assignment must not be consumed: {output}"
    );
}

#[test]
fn rejected_assignment_form_preserves_authored_let_kind_at_rule_boundary() {
    let input = r#"
import { _ as _interop_require_default } from "@swc/helpers/_/_interop_require_default";
let react = require("react");
probe();
react = _interop_require_default(react);
"#;

    assert_eq_normalized(&apply_rule(input), input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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

#[test]
fn typescript_parenthesized_interop_test_restores_default_import() {
    let input = include_str!("fixtures/tslib-interop/generated/default.js");
    let focused = common::render_rule(input, |_| wakaru_core::rules::UnInteropRequireDefault);
    assert!(!focused.contains("__importDefault(require("), "{focused}");
    let output = render(input);
    assert!(!output.contains("__importDefault"), "{output}");
    assert!(output.contains("from \"./provider\""), "{output}");
}

#[test]
fn helper_module_keeps_its_exported_declaration() {
    // The Babel runtime's `interopRequireDefault` helper module itself: the
    // helper function is the module's export, not an unwrapped call site.
    let input = r#"
function q(m) {
    return m && m.__esModule ? m : {
        default: m
    };
}
module.exports = q, module.exports.__esModule = !0, module.exports.default = module.exports;
"#;
    let focused = common::render_rule(input, |_| wakaru_core::rules::UnInteropRequireDefault);
    assert!(focused.contains("function q(m)"), "{focused}");
    assert!(focused.contains("module.exports = q"), "{focused}");

    let output = render(input);
    assert!(output.contains("function q(m)"), "{output}");
    assert!(output.contains("export default q;"), "{output}");
}

#[test]
fn referenced_helper_declaration_survives_call_unwrapping() {
    let input = r#"
function _interopRequireDefault(obj) {
    return obj && obj.__esModule ? obj : { default: obj };
}
var _a = _interopRequireDefault(require("a"));
console.log(_a.default);
exports.helper = _interopRequireDefault;
"#;
    let output = render(input);
    assert!(
        output.contains("function _interopRequireDefault(obj)"),
        "{output}"
    );
    assert!(output.contains("import _a from \"a\";"), "{output}");
    assert!(output.contains("console.log(_a);"), "{output}");
    assert!(
        output.contains("export { _interopRequireDefault as helper };"),
        "{output}"
    );
}

#[test]
fn aliased_helper_declaration_is_not_removed() {
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _a = _interopRequireDefault(require("a"));
var interop = _interopRequireDefault;
console.log(_a.default, interop);
"#;
    let focused = common::render_rule(input, |_| wakaru_core::rules::UnInteropRequireDefault);
    assert!(
        focused.contains("require(\"@babel/runtime/helpers/interopRequireDefault\")"),
        "{focused}"
    );
    assert!(
        focused.contains("var interop = _interopRequireDefault;"),
        "{focused}"
    );
    assert!(focused.contains("var _a = require(\"a\");"), "{focused}");
}

#[test]
fn registered_rule_unwraps_only_helper_runtime_requires() {
    // `_a.default` is the default export only once `_a` becomes a default
    // import; until UnEsm commits to that, the wrapped call stays.
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _extends2 = _interopRequireDefault(require("@babel/runtime/helpers/extends"));
var _a = _interopRequireDefault(require("./a"));
console.log(_extends2.default({}, _a.default));
"#;
    let output = render_pipeline_until(input, "UnInteropRequireDefault");
    assert!(
        output.contains(r#"var _extends2 = require("@babel/runtime/helpers/extends");"#),
        "{output}"
    );
    assert!(
        output.contains(r#"_interopRequireDefault(require("./a"))"#),
        "{output}"
    );
    assert!(output.contains("_a.default"), "{output}");
}

#[test]
fn module_kept_commonjs_keeps_interop_default() {
    // An aliased `exports` keeps the module CommonJS, where `require("./a")`
    // is the whole module: unwrapping would call the module object.
    let input = r#"
"use strict";
var _a = _interopRequireDefault(require("./a"));
function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
var target = exports;
target.run = function () { return _a.default(); };
"#;
    let output = render(input);
    assert!(
        output.contains(r#"_interopRequireDefault(require("./a"))"#),
        "{output}"
    );
    assert!(output.contains("_a.default()"), "{output}");
}

#[test]
fn module_converted_to_esm_unwraps_interop_default() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
var _a = _interopRequireDefault(require("./a"));
function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
function run() { return _a.default(); }
"#;
    let expected = r#"
import _a from "./a";
export { run };
function run() {
    return _a();
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn minimal_level_keeps_interop_default() {
    // `UnEsm` does not convert at `minimal`, so the module stays CommonJS.
    let input = r#"
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
var _a = _interopRequireDefault(require("./a"));
exports.run = function () { return _a.default(); };
"#;
    let output = render_with_level(input, RewriteLevel::Minimal);
    assert!(
        output.contains(r#"_interopRequireDefault(require("./a"))"#),
        "{output}"
    );
    assert!(output.contains("_a.default()"), "{output}");
}

#[test]
fn require_inside_a_function_keeps_interop_default() {
    // `UnEsm` turns only top-level requires into imports. A `require` inside
    // a function stays a `require`, where the interop call means the default
    // export and the unwrapped form the whole module.
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.run = run;
exports.peek = peek;
var _b = _interopRequireDefault(require("./b"));
function _interopRequireDefault(e) { return e && e.__esModule ? e : { default: e }; }
function run() { var _a = _interopRequireDefault(require("./a")); return _a.default() + _b.default(); }
function peek() { return _interopRequireDefault(require("./c")).default; }
"#;
    let output = render(input);
    assert!(output.contains(r#"import _b from "./b""#), "{output}");
    assert!(output.contains("_b()"), "{output}");
    assert!(
        output.contains(r#"_interopRequireDefault(require("./a"))"#),
        "{output}"
    );
    assert!(output.contains("_a.default()"), "{output}");
    assert!(
        output.contains(r#"_interopRequireDefault(require("./c")).default"#),
        "{output}"
    );
    assert!(
        output.contains("function _interopRequireDefault"),
        "{output}"
    );
}

#[test]
fn interop_of_a_top_level_require_binding_inside_a_getter_is_unwrapped() {
    // TypeScript 5.9 `export { default as depDefault } from "./dep.js"`: the
    // getter is a function, but `dep_js_1` is a top-level require that
    // becomes an import, so the call reads that import's default.
    let input = r#"
"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.depDefault = exports.depCount = void 0;
var dep_js_1 = require("./dep.js");
Object.defineProperty(exports, "depCount", { enumerable: true, get: function () { return dep_js_1.depCount; } });
Object.defineProperty(exports, "depDefault", { enumerable: true, get: function () { return __importDefault(dep_js_1).default; } });
"#;
    let output = render(input);
    assert!(!output.contains("exports"), "{output}");
    assert!(!output.contains("__importDefault"), "{output}");
    assert!(output.contains("depDefault"), "{output}");
}
