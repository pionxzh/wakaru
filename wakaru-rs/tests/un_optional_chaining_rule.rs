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

// --- loose equality form: x == null ? undefined : x.prop ---

#[test]
fn transforms_loose_eq_null_member_access() {
    let input = r#"const x = U == null ? undefined : U.userID"#;
    let expected = r#"const x = U?.userID"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_null_method_call() {
    let input = r#"const x = U == null ? undefined : U.getName()"#;
    let expected = r#"const x = U?.getName()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_neq_null_nullish_form() {
    // x != null ? x.prop : undefined  →  x?.prop
    let input = r#"const x = U != null ? U.name : undefined"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_nested_loose_eq_null() {
    // Nested: (x == null ? undefined : x.a) == null ? undefined : (x == null ? undefined : x.a).b
    // After first pass: x?.a, then second nesting would need chaining
    let input = r#"const x = U == null ? undefined : U.a"#;
    let expected = r#"const x = U?.a"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_eq_when_cons_is_not_undefined() {
    let input = r#"const x = U == null ? "default" : U.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_loose_eq_null_in_expression_position() {
    // Inside if condition
    let input = r#"if ((U == null ? undefined : U.message) && true) {}"#;
    let expected = r#"if (U?.message && true) {}"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- loose equality edge cases (from Codex review) ---

#[test]
fn transforms_loose_eq_null_with_void_0_consequent() {
    let input = r#"const x = U == null ? void 0 : U.name"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_null_reversed_operand_order() {
    // null == U instead of U == null
    let input = r#"const x = null == U ? undefined : U.prop"#;
    let expected = r#"const x = U?.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_undefined() {
    // x == undefined is equivalent to x == null in JS
    let input = r#"const x = U == undefined ? undefined : U.name"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- loose equality assignment form ---

#[test]
fn transforms_loose_eq_assignment_member_access() {
    // (tmp = expr) == null ? undefined : tmp.prop  →  expr?.prop
    let input = r#"const x = (n = e.ownerDocument) == null ? undefined : n.defaultView"#;
    let expected = r#"const x = e.ownerDocument?.defaultView"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_assignment_method_call() {
    let input = r#"const x = (t = obj.getRootNode) == null ? undefined : t.call(obj)"#;
    let expected = r#"const x = obj.getRootNode?.call(obj)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_neq_assignment_form() {
    let input = r#"const x = (n = e.body) != null ? n.scrollWidth : undefined"#;
    let expected = r#"const x = e.body?.scrollWidth"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_assignment_with_computed_access() {
    let input = r#"const x = (t = e[n.type]) == null ? undefined : t.duration"#;
    let expected = r#"const x = e[n.type]?.duration"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_logical_and_expression_value_not_converted() {
    let input = r#"x !== null && x !== undefined && x.foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
