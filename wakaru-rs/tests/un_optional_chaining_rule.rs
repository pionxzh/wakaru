mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnOptionalChaining;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnOptionalChaining)
}

#[test]
fn transforms_member_access_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.a"#;
    let expected = r#"obj?.a"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method()"#;
    let expected = r#"obj?.method()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_args() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method(1, 2)"#;
    let expected = r#"obj?.method(1, 2)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_temp_variable_assignment_form() {
    let input = r#"(_a = a) === null || _a === void 0 ? void 0 : _a.b"#;
    let expected = r#"a?.b"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn logical_and_form_stays_as_is() {
    let input = r#"x !== null && x !== void 0 && x.foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_different_variable_in_access() {
    // alt uses `other`, not `obj` — should not transform to optional chain
    let input = r#"obj === null || obj === void 0 ? void 0 : other.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_when_cons_is_not_void() {
    let input = r#"obj === null || obj === void 0 ? "fallback" : obj.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_logical_and_expression_value_not_converted() {
    let input = r#"x !== null && x !== undefined && x.foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
