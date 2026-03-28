mod common;

use common::{assert_eq_normalized, render};

#[test]
fn transforms_unary_plus_ident_to_number_call() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
+x;
+numStr;
"#;
    let expected = r#"
Number(x);
Number(numStr);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_string_concat_with_empty_to_string_call() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
x + "";
"#;
    let expected = r#"
String(x);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn simplifies_string_literal_concat_with_empty_string() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
const x = 'str' + '';
"#;
    let expected = r#"
const x = 'str';
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_all_holes_array_to_array_call() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
const a = [,,,];
const b = [,];
"#;
    let expected = r#"
const a = Array(3);
const b = Array(1);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_empty_array() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
const x = [];
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_unary_plus_on_non_ident() {
    // Reused from packages/unminify/src/transformations/__tests__/un-type-constructor.spec.ts
    let input = r#"
const a = +"42";
const b = +42;
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}
