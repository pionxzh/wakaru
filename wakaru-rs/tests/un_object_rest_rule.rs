mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_rs::rules::UnObjectRest;

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
    assert!(!output.contains("indexOf"), "IIFE should be removed: {output}");
    assert!(output.contains("...rest"), "should have rest: {output}");
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
    let rule_output = render_rule(input, |_| UnObjectRest);
    assert!(
        rule_output.contains("a: x"),
        "rule should emit aliased destructuring {{ a: x }}: {rule_output}"
    );

    // Full pipeline: SmartInline runs after and may inline x
    let pipeline_output = render(input);
    assert!(
        !pipeline_output.contains("indexOf"),
        "IIFE should be removed: {pipeline_output}"
    );
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
    assert!(!output.contains("indexOf"), "IIFE should be removed: {output}");
    assert!(output.contains("...rest"), "should have rest: {output}");
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
    let rule_output = render_rule(input, |_| UnObjectRest);
    assert!(
        !rule_output.contains("indexOf"),
        "IIFE should be removed: {rule_output}"
    );
    assert!(
        rule_output.contains("...d"),
        "should have rest destructuring: {rule_output}"
    );
    // t and n should be absorbed into the rest destructuring
    assert!(
        rule_output.contains("to: t") || rule_output.contains("to,"),
        "should have 'to' in destructuring: {rule_output}"
    );
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
    let rule_output = render_rule(input, |_| UnObjectRest);
    assert!(
        !rule_output.contains("indexOf"),
        "IIFE should be removed: {rule_output}"
    );
    assert!(
        rule_output.contains("...d"),
        "should have rest destructuring: {rule_output}"
    );
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
    let rule_output = render_rule(input, |_| UnObjectRest);
    assert!(
        !rule_output.contains("indexOf"),
        "IIFE should be removed: {rule_output}"
    );
    // The preceding destructuring should be absorbed
    let destructuring_count = rule_output.matches("= e").count();
    assert_eq!(destructuring_count, 1, "should have exactly one destructuring from e: {rule_output}");
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
    let rule_output = render_rule(input, |_| UnObjectRest);
    assert!(
        !rule_output.contains("indexOf"),
        "IIFE should be removed even with shadowed param: {rule_output}"
    );
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
    assert!(output.contains("sideEffect"), "side effect must not be dropped: {output}");
    assert!(output.contains("fallback"), "fallback return must not be dropped: {output}");
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
    // Should NOT produce `const { replace, ...rest } = props` since `replace` already exists
    let replace_count = output.matches("const replace").count();
    assert!(replace_count <= 1, "should not duplicate const replace: {output}");
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
    // Should use _replace_1 (or similar) to avoid colliding with existing _replace
    assert!(result.contains("_replace_1") || result.contains("_replace_2"),
        "should generate a non-colliding alias, got:\n{}", result);
    assert!(!result.contains("replace: _replace,") && !result.contains("replace: _replace }"),
        "must not use bare _replace (collides with existing binding):\n{}", result);
}
