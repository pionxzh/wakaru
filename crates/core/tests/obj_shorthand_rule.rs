mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ObjShorthand;

fn apply(input: &str) -> String {
    render_rule(input, |_| ObjShorthand)
}

#[test]
fn same_name_ident_becomes_shorthand() {
    let input = r#"const obj = {foo: foo};"#;
    let expected = r#"const obj = {foo};"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn multiple_properties_converted() {
    let input = r#"const obj = {a: a, b: b, c: c};"#;
    let expected = r#"const obj = {a, b, c};"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mixed_shorthand_and_renamed() {
    let input = r#"const obj = {x: x, y: 1};"#;
    let expected = r#"const obj = {x, y: 1};"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn different_key_and_value_not_converted() {
    let input = r#"const obj = {foo: bar};"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn string_key_not_converted() {
    let input = r#"const obj = {"foo": foo};"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn numeric_key_not_converted() {
    let input = r#"const obj = {0: zero};"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn computed_key_not_converted() {
    let input = r#"const obj = {[foo]: foo};"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn non_ident_value_not_converted() {
    let input = r#"const obj = {foo: foo.bar};"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn object_pattern_same_name_binding_becomes_shorthand() {
    let input = r#"function pick({ name: name } = {}) { return name; }"#;
    let expected = r#"function pick({ name } = {}) { return name; }"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_pattern_same_name_default_becomes_shorthand_default() {
    let input = r#"function pick({ name: name, age: age = 0 } = {}) { return use(name, age); }"#;
    let expected = r#"function pick({ name, age = 0 } = {}) { return use(name, age); }"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_object_pattern_default_becomes_shorthand_default() {
    let input = r#"function nested({ outer: { value: value = fallbackValue } = {} } = {}) { return use(value); }"#;
    let expected =
        r#"function nested({ outer: { value = fallbackValue } = {} } = {}) { return use(value); }"#;
    assert_eq_normalized(&apply(input), expected);
}
