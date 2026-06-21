mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::facts::{
    HelperExportFact, HelperKind, ModuleFacts, ModuleFactsMap, TypeScriptHelperExportFact,
    TypeScriptHelperKind,
};
use wakaru_core::rules::UnObjectRest;

#[test]
fn basic_object_without_properties_iife() {
    // Simplest case: IIFE with no preceding destructuring
    // Excluded keys get _prefix aliases to avoid collisions, but SmartInline
    // removes unused bindings so they appear as shorthand in final output
    let input = r#"
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["a", "b"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn with_preceding_destructuring() {
    // Preceding destructuring from same source — merge into one
    // "replace" has no preceding binding so gets _replace alias
    let input = r#"
const { to, innerRef } = props;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["replace", "to", "innerRef"]);
use(to, innerRef, rest);
"#;
    let expected = r#"
const { replace: _replace, to, innerRef, ...rest } = props;
use(to, innerRef, rest);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_tslib_named_rest_import() {
    let input = r#"
import { __rest } from "tslib";
const a = props.a;
const rest = __rest(props, ["a"]);
use(a, rest);
"#;
    let expected = r#"
const { a, ...rest } = props;
use(a, rest);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_cross_module_ts_rest_helper_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__rest".into(),
                local: Some("__rest".into()),
                kind: TypeScriptHelperKind::Rest,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { __rest } from "./helpers.js";
var label = props.label, rest = __rest(props, ["label"]);
use(label, rest);
"#;
    let expected = r#"
import { __rest } from "./helpers.js";
const { label, ...rest } = props;
use(label, rest);
"#;
    assert_eq_normalized(
        &render_rule(input, |mark| UnObjectRest::new_with_facts(mark, &facts)),
        expected,
    );
}

#[test]
fn handles_cross_module_ts_rest_helper_namespace_fact() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            ts_helper_exports: vec![TypeScriptHelperExportFact {
                exported: "__rest".into(),
                local: Some("__rest".into()),
                kind: TypeScriptHelperKind::Rest,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import * as helpers from "./helpers.js";
var label = props.label, rest = helpers.__rest(props, ["label"]);
use(label, rest);
"#;
    let expected = r#"
import * as helpers from "./helpers.js";
const { label, ...rest } = props;
use(label, rest);
"#;
    assert_eq_normalized(
        &render_rule(input, |mark| UnObjectRest::new_with_facts(mark, &facts)),
        expected,
    );
}

#[test]
fn handles_tslib_namespace_rest_require() {
    let input = r#"
var tslib_1 = require("tslib");
const a = props.a;
const rest = tslib_1.__rest(props, ["a"]);
use(a, rest);
"#;
    let expected = r#"
import tslib_1 from "tslib";
const { a, ...rest } = props;
use(a, rest);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn with_preceding_prop_access() {
    // const x = obj.prop before IIFE — absorbed into rest destructuring with alias
    let input = r#"
const x = props.a;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["a", "b"]);
use(x, rest);
"#;
    // Test rule in isolation to verify alias handling
    let rule_output = render_rule(input, UnObjectRest::new);
    insta::assert_snapshot!("with_preceding_prop_access_rule", rule_output);

    // Full pipeline: SmartInline runs after and may inline x
    let pipeline_output = render(input);
    insta::assert_snapshot!("with_preceding_prop_access_pipeline", pipeline_output);
}

#[test]
fn plain_spreads_do_not_trigger_elided_rest_reattachment() {
    let input = r#"
var missing;
var value = {
    ...source
};
use(missing, value);
"#;
    assert_eq_normalized(&render_rule(input, UnObjectRest::new), input.trim());
}

#[test]
fn with_bare_access() {
    // props.replace; (bare access, no binding) is absorbed. The long `_replace`
    // alias stays descriptive enough, while short `_to` can collapse to shorthand.
    let input = r#"
props.replace;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["replace", "to"]);
use(rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn multi_declarator_comma_separated() {
    // Babel output: var t = e.to, n = e.exact, d = IIFE(e, ["to","exact"]), p = expr
    let input = r#"
const f = (e) => {
    var t = e.to, n = e.exact, d = (function(e, t) {
        var n = {};
        for (var r in e) {
            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
        }
        return n;
    })(e, ["to", "exact"]), p = "hello";
    use(t, n, d, p);
};
"#;
    let rule_output = render_rule(input, UnObjectRest::new);
    insta::assert_snapshot!(rule_output);
}

#[test]
fn with_many_keys_destructuring() {
    // NavLink-like pattern: large destructuring before IIFE with same keys
    let input = r#"
const f = (e) => {
    const { to, exact, strict } = e;
    const d = ((e, t) => {
        const n = {};
        for (const r in e) {
            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
        }
        return n;
    })(e, ["to", "exact", "strict"]);
    use(to, exact, strict, d);
};
"#;
    let rule_output = render_rule(input, UnObjectRest::new);
    insta::assert_snapshot!(rule_output);
}

#[test]
fn with_string_key_in_destructuring() {
    // "aria-current" is a string key — should still be absorbed
    let input = r#"
const f = (e) => {
    const { to, "aria-current": ariaCurrent } = e;
    const d = ((e, t) => {
        const n = {};
        for (const r in e) {
            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
        }
        return n;
    })(e, ["to", "aria-current"]);
    use(to, ariaCurrent, d);
};
"#;
    let rule_output = render_rule(input, UnObjectRest::new);
    insta::assert_snapshot!(rule_output);
}

#[test]
fn with_shadowed_iife_param() {
    // IIFE param shadows outer variable — should still detect
    let input = r#"
const f = (e) => {
    const { to } = e;
    const d = ((e, t) => {
        const n = {};
        for (const r in e) {
            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
        }
        return n;
    })(e, ["to", "extra"]);
    use(to, d);
};
"#;
    let rule_output = render_rule(input, UnObjectRest::new);
    insta::assert_snapshot!(rule_output);
}

#[test]
fn no_match_arbitrary_for_in_iife() {
    // P1 regression: IIFE with for-in but no indexOf/hasOwnProperty must NOT be matched
    let input = r#"
const rest = ((e, t) => {
    const n = {};
    for (const r in e) { sideEffect(r); }
    return fallback;
})(props, ["a"]);
use(rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn no_duplicate_binding_for_excluded_key() {
    // P1 regression: excluded key with existing binding in scope must not create duplicate const
    let input = r#"
const replace = external;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["replace"]);
use(replace, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn function_form_helper() {
    // Function expression form (not arrow)
    let input = r#"
const rest = (function(e, t) {
    var n = {};
    for (var r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["x"]);
"#;
    let expected = r#"
const { x, ...rest } = props;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn alias_does_not_collide_with_existing_binding() {
    // When _replace already exists in scope, the generated alias should not duplicate it
    let input = r#"
const _replace = sentinel;
const rest = function(e, t) {
    var n = {};
    for (var r in e) t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    return n;
}(props, ["replace"]);
"#;
    let result = render(input);
    insta::assert_snapshot!(result);
}

// ── Named function helper (non-IIFE) ─────────────────────────────

#[test]
fn named_owp_helper_basic() {
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const x = props.a;
const rest = m(props, ["a", "b"]);
use(x, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_swc_external_import() {
    let input = r#"
import { _ as _object_without_properties } from "@swc/helpers/_/_object_without_properties";
var name = app_info.name, rest_info = _object_without_properties(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_cross_module_default_object_helper_member() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            default_object_helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("rest".into()),
                kind: HelperKind::ObjectWithoutProperties,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import helpers from "./helpers.js";
var name = app_info.name, rest_info = helpers._(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
import helpers from "./helpers.js";
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(
        &render_rule(input, |mark| UnObjectRest::new_with_facts(mark, &facts)),
        expected,
    );
}

#[test]
fn named_owp_cross_module_direct_helper_keeps_import() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "rest".into(),
                local: Some("rest".into()),
                kind: HelperKind::ObjectWithoutProperties,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { rest as objectWithoutProperties } from "./helpers.js";
var name = app_info.name, rest_info = objectWithoutProperties(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
import { rest as objectWithoutProperties } from "./helpers.js";
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(
        &render_rule(input, |mark| UnObjectRest::new_with_facts(mark, &facts)),
        expected,
    );
}

#[test]
fn named_owp_helper_with_preceding_destructuring() {
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const { to, exact } = props;
const rest = m(props, ["to", "exact", "strict"]);
use(to, exact, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_multiple_call_sites() {
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const rest1 = m(a, ["x"]);
const rest2 = m(b, ["y", "z"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_absorbs_default_pairs() {
    // Two-statement pattern: extraction + ternary default → destructuring with default
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const a = t;
const a_name = a.name;
const x = a_name === undefined ? "default" : a_name;
const a_value = a.value;
const y = a_value === undefined ? 42 : a_value;
const rest = m(a, ["name", "value"]);
use(x, y, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_undefined_default_omitted() {
    // When default is `undefined`, no explicit default is needed in destructuring
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const a_prop = obj.prop;
const x = a_prop === undefined ? undefined : a_prop;
const rest = m(obj, ["prop"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn iife_absorbs_default_pairs() {
    let input = r#"
const a_name = props.name;
const x = a_name === undefined ? "default" : a_name;
const a_value = props.value;
const y = a_value === undefined ? 42 : a_value;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["name", "value"]);
use(x, y, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn iife_absorbs_boolean_defaults() {
    let input = r#"
const a_enabled = props.enabled;
const x = a_enabled === undefined || a_enabled;
const a_hidden = props.hidden;
const y = a_hidden !== undefined && a_hidden;
const rest = ((e, t) => {
    const n = {};
    for (const r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
})(props, ["enabled", "hidden"]);
use(x, y, rest);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_boolean_default_true() {
    // `tmp === undefined || tmp` is Babel's transpilation of `{ prop = true }`
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const a_enabled = obj.enabled;
const x = a_enabled === undefined || a_enabled;
const rest = m(obj, ["enabled"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_boolean_default_false() {
    // `tmp !== undefined && tmp` is Babel's transpilation of `{ prop = false }`
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const a_withRef = obj.withRef;
const R = a_withRef !== undefined && a_withRef;
const rest = m(obj, ["withRef"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_inside_function_body() {
    // Named helper defined at module level, called inside a function
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (!(t.indexOf(i[r]) >= 0)) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
export function foo(t = {}) {
    const a_name = t.name;
    const x = a_name === undefined ? "default" : a_name;
    const rest = m(t, ["name"]);
    use(x, rest);
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_decompiled_object_keys_form() {
    // Exact shape from webpack4 module-23 after Stage 1 normalization:
    // separate temp vars, key assigned in loop, copy uses temp var
    let input = r#"
function m(e, t) {
    if (e == null) {
        return {};
    }
    let n;
    let r;
    const o = {};
    const i = Object.keys(e);
    for(r = 0; r < i.length; r++){
        n = i[r];
        if (!(t.indexOf(n) >= 0)) {
            o[n] = e[n];
        }
    }
    return o;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_minified_or_guard_object_keys_form() {
    let input = r#"
function m(e, t) {
    if (e == null) {
        return {};
    }
    let n;
    let r;
    const o = {};
    const i = Object.keys(e);
    for(r = 0; r < i.length; r++){
        n = i[r];
        t.indexOf(n) >= 0 || (o[n] = e[n]);
    }
    return o;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_in_variant() {
    // The for-in + hasOwnProperty variant as a named function
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r in e) {
        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_babel_loose_continue_form() {
    let input = r#"
function _objectWithoutPropertiesLoose(r, e) {
    if (null == r) return {};
    var t = {};
    for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
        if (-1 !== e.indexOf(n)) continue;
        t[n] = r[n];
    }
    return t;
}
const { name } = app_info, rest_info = _objectWithoutPropertiesLoose(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_babel_spec_wrapper_form() {
    let input = r#"
function _objectWithoutProperties(e, t) {
    if (null == e) return {};
    var o, r, i = _objectWithoutPropertiesLoose(e, t);
    if (Object.getOwnPropertySymbols) {
        var n = Object.getOwnPropertySymbols(e);
        for (r = 0; r < n.length; r++) o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
    }
    return i;
}
function _objectWithoutPropertiesLoose(r, e) {
    if (null == r) return {};
    var t = {};
    for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
        if (-1 !== e.indexOf(n)) continue;
        t[n] = r[n];
    }
    return t;
}
const { name } = app_info, rest_info = _objectWithoutProperties(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_spec_wrapper_with_hoisted_exclusion_array() {
    let input = r#"
var _excluded = ["name"];
function _objectWithoutProperties(e, t) {
    if (null == e) return {};
    var o, r, i = _objectWithoutPropertiesLoose(e, t);
    if (Object.getOwnPropertySymbols) {
        var n = Object.getOwnPropertySymbols(e);
        for (r = 0; r < n.length; r++) o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
    }
    return i;
}
function _objectWithoutPropertiesLoose(r, e) {
    if (null == r) return {};
    var t = {};
    for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
        if (-1 !== e.indexOf(n)) continue;
        t[n] = r[n];
    }
    return t;
}
const rest_info = _objectWithoutProperties(app_info, _excluded);
use(rest_info);
"#;
    let expected = r#"
const { name: _name, ...rest_info } = app_info;
use(rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn iife_spec_wrapper_with_hoisted_exclusion_array() {
    let input = r#"
const excluded = ["label"];
const component = (props) => {
    const label = props.label;
    const rest = ((e, t) => {
        if (e == null) return {};
        var o, r, i = ((r, e) => {
            if (r == null) return {};
            var t = {};
            for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
                if (-1 !== e.indexOf(n)) continue;
                t[n] = r[n];
            }
            return t;
        })(e, t);
        if (Object.getOwnPropertySymbols) {
            var n = Object.getOwnPropertySymbols(e);
            for (r = 0; r < n.length; r++) o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
        }
        return i;
    })(props, excluded);
    use(label, rest);
};
"#;

    let expected = r#"
const component = ({ label, ...rest }) => {
    use(label, rest);
};
"#;

    assert_eq_normalized(&render(input), expected);
}

#[test]
fn keeps_defaulted_prop_alias_used_as_jsx_tag() {
    let input = r#"
const component = (props) => {
    const asProp = props.as;
    const Tag = asProp === undefined ? "div" : asProp;
    const rest = ((e, t) => {
        if (e == null) return {};
        var o, r, i = ((r, e) => {
            if (r == null) return {};
            var t = {};
            for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
                if (-1 !== e.indexOf(n)) continue;
                t[n] = r[n];
            }
            return t;
        })(e, t);
        if (Object.getOwnPropertySymbols) {
            var n = Object.getOwnPropertySymbols(e);
            for (r = 0; r < n.length; r++) o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
        }
        return i;
    })(props, ["as"]);
    return <Tag {...rest}/>;
};
"#;

    let output = render(input);
    assert!(
        output.contains("const Tag = asProp === undefined ? \"div\" : asProp"),
        "defaulted JSX tag alias should stay intact:\n{output}"
    );
    assert!(
        output.contains("return <Tag {...rest}/>"),
        "JSX tag should stay capitalized:\n{output}"
    );
    assert!(
        output.contains("const rest ="),
        "object-rest helper should not fold when it would remove the JSX tag alias:\n{output}"
    );
}

#[test]
fn keeps_defaulted_prop_alias_used_as_create_element_tag() {
    let input = r#"
const component = (props) => {
    const asProp = props.as;
    const Tag = asProp === undefined ? "div" : asProp;
    const rest = ((e, t) => {
        if (e == null) return {};
        var o, r, i = ((r, e) => {
            if (r == null) return {};
            var t = {};
            for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
                if (-1 !== e.indexOf(n)) continue;
                t[n] = r[n];
            }
            return t;
        })(e, t);
        if (Object.getOwnPropertySymbols) {
            var n = Object.getOwnPropertySymbols(e);
            for (r = 0; r < n.length; r++) o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
        }
        return i;
    })(props, ["as"]);
    return React.createElement(Tag, rest);
};
"#;

    let output = render(input);
    assert!(
        output.contains("const Tag = asProp === undefined ? \"div\" : asProp"),
        "defaulted createElement tag alias should stay intact:\n{output}"
    );
    assert!(
        output.contains("<Tag {...rest}/>"),
        "createElement tag should keep the alias binding:\n{output}"
    );
    assert!(
        output.contains("const rest ="),
        "object-rest helper should not fold when it would remove the createElement tag alias:\n{output}"
    );
}

#[test]
fn swc_numeric_namespace_object_rest() {
    let input = r#"
const helpers = require(12345);
const picked = input.picked;
const rest = helpers._T(input, ["picked"]);
use(picked, rest);
"#;
    let expected = r#"
const { picked, ...rest } = input;
use(picked, rest);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_shadowed_numeric_require_object_rest_namespace() {
    let input = r#"
function require(id) {
    return load(id);
}
const helpers = require(12345);
const picked = input.picked;
const rest = helpers._T(input, ["picked"]);
use(picked, rest);
"#;

    let output = render(input);
    assert!(
        output.contains("helpers._T(input"),
        "shadowed numeric require namespace must not be rewritten:\n{output}"
    );
    assert!(
        !output.contains("...rest"),
        "shadowed numeric require namespace must not produce object rest:\n{output}"
    );
}

#[test]
fn named_owp_helper_preserves_destructuring_defaults() {
    let input = r#"
function _objectWithoutPropertiesLoose(r, e) {
    if (null == r) return {};
    var t = {};
    for (var n in r) if ({}.hasOwnProperty.call(r, n)) {
        if (-1 !== e.indexOf(n)) continue;
        t[n] = r[n];
    }
    return t;
}
const { name: app_name, version = fallback_version } = app_info,
  rest_info = _objectWithoutPropertiesLoose(app_info, ["name", "version"]);
use(app_name, version, rest_info);
"#;
    let expected = r#"
const { name: app_name, version = fallback_version, ...rest_info } = app_info;
use(app_name, version, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_ts_rest_helper() {
    let input = r#"
var __rest = (this && this.__rest) || function (s, e) {
    var t = {};
    for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p) && e.indexOf(p) < 0)
        t[p] = s[p];
    if (s != null && typeof Object.getOwnPropertySymbols === "function")
        for (var i = 0, p = Object.getOwnPropertySymbols(s); i < p.length; i++) {
            if (e.indexOf(p[i]) < 0 && Object.prototype.propertyIsEnumerable.call(s, p[i]))
                t[p[i]] = s[p[i]];
        }
    return t;
};
var name = app_info.name, rest_info = __rest(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_assignment_with_preceding_assignments() {
    let input = r#"
var __rest = (this && this.__rest) || function (s, e) {
    var t = {};
    for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p) && e.indexOf(p) < 0)
        t[p] = s[p];
    if (s != null && typeof Object.getOwnPropertySymbols === "function")
        for (var i = 0, p = Object.getOwnPropertySymbols(s); i < p.length; i++) {
            if (e.indexOf(p[i]) < 0 && Object.prototype.propertyIsEnumerable.call(s, p[i]))
                t[p[i]] = s[p[i]];
        }
    return t;
};
let id;
let token;
let options;
id = source.id;
token = source.token;
options = __rest(source, ["id", "token"]);
use(id, token, options);
"#;
    let expected = r#"
let id;
let token;
let options;
({ id, token, ...options } = source);
use(id, token, options);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_assignment_absorbs_source_initializing_member_access() {
    let input = r#"
var __rest = (this && this.__rest) || function (s, e) {
    var t = {};
    for (var p in s) if (Object.prototype.hasOwnProperty.call(s, p) && e.indexOf(p) < 0)
        t[p] = s[p];
    if (s != null && typeof Object.getOwnPropertySymbols === "function")
        for (var i = 0, p = Object.getOwnPropertySymbols(s); i < p.length; i++) {
            if (e.indexOf(p[i]) < 0 && Object.prototype.propertyIsEnumerable.call(s, p[i]))
                t[p[i]] = s[p[i]];
        }
    return t;
};
let source;
let id;
let token;
let options;
let tmp;
tmp = config != null ? config : fallback();
id = (source = tmp).id;
token = source.token;
options = __rest(source, ["id", "token"]);
use(source, id, token, options);
"#;
    let expected = r#"
let source;
let id;
let token;
let options;
let tmp;
tmp = config ?? fallback();
source = tmp;
({ id, token, ...options } = source);
use(source, id, token, options);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_assignment_absorbs_source_initializing_member_access_in_function_body() {
    let input = r#"
var excluded = ["id", "token"];
function objectWithoutProperties(source, excluded) {
    if (source == null) return {};
    var key, index, rest = objectWithoutPropertiesLoose(source, excluded);
    if (Object.getOwnPropertySymbols) {
        var symbols = Object.getOwnPropertySymbols(source);
        for (index = 0; index < symbols.length; index++)
            key = symbols[index], -1 === excluded.indexOf(key) && {}.propertyIsEnumerable.call(source, key) && (rest[key] = source[key]);
    }
    return rest;
}
function objectWithoutPropertiesLoose(source, excluded) {
    if (source == null) return {};
    var rest = {};
    for (var key in source) if ({}.hasOwnProperty.call(source, key)) {
        if (-1 !== excluded.indexOf(key)) continue;
        rest[key] = source[key];
    }
    return rest;
}
const load_user = async (config) => {
    let source;
    let id;
    let token;
    let options;
    let tmp;
    tmp = config != null ? config : await load_config();
    id = (source = tmp).id;
    token = source.token;
    options = objectWithoutProperties(source, excluded);
    return fetch_user(id, token, options);
};
use(load_user);
"#;
    let expected = r#"
const load_user = async (config) => {
    let source;
    let id;
    let token;
    let options;
    let tmp;
    tmp = config ?? await load_config();
    source = tmp;
    ({ id, token, ...options } = source);
    return fetch_user(id, token, options);
};
use(load_user);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_swc_rest_helper() {
    let input = r#"
function _object_without_properties(source, excluded) {
    if (source == null) return {};
    var target = {}, sourceKeys, key, i;
    if (typeof Reflect !== "undefined" && Reflect.ownKeys) {
        sourceKeys = Reflect.ownKeys(Object(source));
        for (i = 0; i < sourceKeys.length; i++) {
            key = sourceKeys[i];
            if (excluded.indexOf(key) >= 0) continue;
            if (!Object.prototype.propertyIsEnumerable.call(source, key)) continue;
            target[key] = source[key];
        }
        return target;
    }
    target = _object_without_properties_loose(source, excluded);
    if (Object.getOwnPropertySymbols) {
        sourceKeys = Object.getOwnPropertySymbols(source);
        for (i = 0; i < sourceKeys.length; i++) {
            key = sourceKeys[i];
            if (excluded.indexOf(key) >= 0) continue;
            if (!Object.prototype.propertyIsEnumerable.call(source, key)) continue;
            target[key] = source[key];
        }
    }
    return target;
}
function _object_without_properties_loose(source, excluded) {
    if (source == null) return {};
    var target = {}, sourceKeys = Object.getOwnPropertyNames(source), key, i;
    for (i = 0; i < sourceKeys.length; i++) {
        key = sourceKeys[i];
        if (excluded.indexOf(key) >= 0) continue;
        if (!Object.prototype.propertyIsEnumerable.call(source, key)) continue;
        target[key] = source[key];
    }
    return target;
}
var name = app_info.name, rest_info = _object_without_properties(app_info, ["name"]);
use(name, rest_info);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
use(name, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_esbuild_rest_helper() {
    let input = r#"
var __getOwnPropSymbols = Object.getOwnPropertySymbols;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __propIsEnum = Object.prototype.propertyIsEnumerable;
var __objRest = (source, exclude) => {
    var target = {};
    for (var prop in source)
        if (__hasOwnProp.call(source, prop) && exclude.indexOf(prop) < 0)
            target[prop] = source[prop];
    if (source != null && __getOwnPropSymbols)
        for (var prop of __getOwnPropSymbols(source)) {
            if (exclude.indexOf(prop) < 0 && __propIsEnum.call(source, prop))
                target[prop] = source[prop];
        }
    return target;
};
const _a = app_info, { name, version = fallback_version } = _a, rest_info = __objRest(_a, ["name", "version"]);
use(name, version, rest_info);
"#;
    let expected = r#"
const { name, version = fallback_version, ...rest_info } = app_info;
use(name, version, rest_info);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn reattaches_terser_elided_swc_rest_binding_from_later_spread() {
    let input = r#"
function _object_without_properties(source, excluded) {
    if (source == null) return {};
    var target = {}, sourceKeys, key, i;
    if (typeof Reflect !== "undefined" && Reflect.ownKeys) {
        sourceKeys = Reflect.ownKeys(Object(source));
        for (i = 0; i < sourceKeys.length; i++)
            key = sourceKeys[i], excluded.indexOf(key) >= 0 || Object.prototype.propertyIsEnumerable.call(source, key) && (target[key] = source[key]);
        return target;
    }
    target = _object_without_properties_loose(source, excluded);
    if (Object.getOwnPropertySymbols)
        for (sourceKeys = Object.getOwnPropertySymbols(source), i = 0; i < sourceKeys.length; i++)
            key = sourceKeys[i], excluded.indexOf(key) >= 0 || Object.prototype.propertyIsEnumerable.call(source, key) && (target[key] = source[key]);
    return target;
}
function _object_without_properties_loose(source, excluded) {
    if (source == null) return {};
    var target = {}, sourceKeys = Object.getOwnPropertyNames(source), key, i;
    for (i = 0; i < sourceKeys.length; i++)
        key = sourceKeys[i], excluded.indexOf(key) >= 0 || Object.prototype.propertyIsEnumerable.call(source, key) && (target[key] = source[key]);
    return target;
}
var name = app_info.name, rest_info, out = { ..._object_without_properties(app_info, ["name"]), name };
use(out);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
const out = { ...rest_info, name };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn reattaches_terser_elided_esbuild_rest_binding_from_later_spread() {
    let input = r#"
var __getOwnPropSymbols = Object.getOwnPropertySymbols;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __propIsEnum = Object.prototype.propertyIsEnumerable;
var __objRest = (source, exclude) => {
    var target = {};
    for (var prop in source)
        if (__hasOwnProp.call(source, prop) && exclude.indexOf(prop) < 0)
            target[prop] = source[prop];
    if (source != null && __getOwnPropSymbols)
        for (var prop of __getOwnPropSymbols(source))
            if (exclude.indexOf(prop) < 0 && __propIsEnum.call(source, prop))
                target[prop] = source[prop];
    return target;
};
const _a = app_info, { name } = _a, rest_info = undefined, out = { ...__objRest(_a, ["name"]), name };
use(out);
"#;
    let expected = r#"
const { name, ...rest_info } = app_info;
const out = { ...rest_info, name };
use(out);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn named_owp_helper_for_in_requires_guarded_copy() {
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r in e) {
        t.indexOf(r);
        Object.prototype.hasOwnProperty.call(e, r);
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_in_rejects_unguarded_copy() {
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r in e) {
        t.indexOf(r);
        Object.prototype.hasOwnProperty.call(e, r);
        n[r] = e[r];
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_requires_copy() {
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r = 0; r < 1; r++) {
        t.indexOf("a");
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_rejects_unguarded_copy() {
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r = 0; r < 1; r++) {
        t.indexOf(r);
        n[r] = e[r];
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_rejects_inverted_if_guard() {
    let input = r#"
function m(e, t) {
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        if (t.indexOf(i[r]) >= 0) {
            n[i[r]] = e[i[r]];
        }
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_rejects_inverted_or_guard() {
    let input = r#"
function m(e, t) {
    if (e == null) return {};
    var n = {};
    var i = Object.keys(e);
    for (var r = 0; r < i.length; r++) {
        t.indexOf(i[r]) < 0 || (n[i[r]] = e[i[r]]);
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn named_owp_helper_for_in_rejects_inverted_guarded_copy() {
    let input = r#"
function m(e, t) {
    var n = {};
    for (var r in e) {
        Object.prototype.hasOwnProperty.call(e, r) && t.indexOf(r) >= 0 && (n[r] = e[r]);
    }
    return n;
}
const rest = m(obj, ["a"]);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn preserves_unrelated_imported_define_property_helper() {
    let input = r#"
import d from "@babel/runtime/helpers/defineProperty";
import owp from "@babel/runtime/helpers/objectWithoutProperties";
var name = app_info.name, rest_info = owp(app_info, ["name"]);
use(name, rest_info);
"#;
    let output = render(input);
    assert!(
        output.contains("...rest_info"),
        "should destructure: {output}"
    );
    assert!(
        output.contains("@babel/runtime/helpers/defineProperty"),
        "unrelated defineProperty import should survive: {output}"
    );
}
