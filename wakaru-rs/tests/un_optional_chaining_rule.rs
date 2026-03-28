mod common;

use common::{assert_eq_normalized, render};

#[test]
fn transforms_member_access_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.a"#;
    let expected = r#"obj?.a"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method()"#;
    let expected = r#"obj?.method()"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_args() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method(1, 2)"#;
    let expected = r#"obj?.method(1, 2)"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_temp_variable_assignment_form() {
    let input = r#"(_a = a) === null || _a === void 0 ? void 0 : _a.b"#;
    let expected = r#"a?.b"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn logical_and_form_stays_as_is() {
    let input = r#"x !== null && x !== void 0 && x.foo"#;
    let output = render(input);
    assert_eq_normalized(&output, r#"x !== null && x !== undefined && x.foo"#);
}

#[test]
fn does_not_transform_different_variable_in_access() {
    // alt uses `other`, not `obj` — should not transform to optional chain
    let input = r#"obj === null || obj === void 0 ? void 0 : other.prop"#;
    let output = render(input);
    // RemoveVoid converts `void 0` to `undefined`; no further transformation applies
    assert_eq_normalized(&output, r#"obj === null || obj === undefined ? undefined : other.prop"#);
}

#[test]
fn does_not_transform_when_cons_is_not_void() {
    // The consequent is "fallback", not void 0 — not an optional chaining pattern.
    // It is also not a simple nullish coalescing pattern since alt is obj.prop not obj.
    // RemoveVoid converts void 0 to undefined; no further transformation applies.
    let input = r#"obj === null || obj === void 0 ? "fallback" : obj.prop"#;
    let output = render(input);
    assert_eq_normalized(&output, r#"obj === null || obj === undefined ? "fallback" : obj.prop"#);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_logical_and_expression_value_not_converted() {
    let input = r#"x !== null && x !== undefined && x.foo"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}
