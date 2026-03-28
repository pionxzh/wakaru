mod common;

use wakaru_rs::rules::UnNullishCoalescing;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnNullishCoalescing)
}

#[test]
fn transforms_not_null_and_not_undefined_ternary() {
    let input = r#"foo !== null && foo !== void 0 ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_null_or_undefined_ternary_flipped() {
    let input = r#"foo === null || foo === void 0 ? "bar" : foo"#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_temp_variable_assignment_in_condition() {
    let input = r#"(_ref = foo) !== null && _ref !== void 0 ? _ref : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_undefined_identifier_instead_of_void_0() {
    let input = r#"foo !== null && foo !== undefined ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_non_null_check_ternary() {
    let input = r#"foo ? bar : baz"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_mismatched_variable() {
    let input = r#"foo !== null && bar !== void 0 ? foo : "baz""#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

