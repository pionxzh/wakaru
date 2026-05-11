mod common;

use common::{assert_eq_normalized, render, render_rule};
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
fn with_bare_access() {
    // props.replace; (bare access, no binding) — absorbed, "to" gets _prefix since no binding
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
