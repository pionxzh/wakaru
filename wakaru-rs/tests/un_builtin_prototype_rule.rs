mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnBuiltinPrototype;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnBuiltinPrototype)
}

#[test]
fn replaces_array_instance_with_prototype() {
    let input = r#"
[].splice.apply(a, [1, 2]);
"#;
    let expected = r#"
Array.prototype.splice.apply(a, [1, 2]);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_number_instance_with_prototype() {
    let input = r#"
0..toFixed.call(Math.PI, 2);
"#;
    let expected = r#"
Number.prototype.toFixed.call(Math.PI, 2);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_object_instance_with_prototype() {
    let input = r#"
({}).hasOwnProperty.call(d, "foo");
"#;
    let expected = r#"
Object.prototype.hasOwnProperty.call(d, "foo");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_string_instance_with_prototype() {
    let input = r#"
"".indexOf.call(e, "bar");
"#;
    let expected = r#"
String.prototype.indexOf.call(e, "bar");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_regexp_instance_with_prototype() {
    let input = r#"
/t/.test.call(/foo/, "bar");
"#;
    let expected = r#"
RegExp.prototype.test.call(/foo/, "bar");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_function_instance_with_prototype() {
    let input = r#"
(function() {}).call.apply(console.log, [console, "foo"]);
"#;
    let expected = r#"
Function.prototype.call.apply(console.log, [console, "foo"]);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn replaces_arrow_function_instance_with_prototype() {
    let input = r#"
(() => {}).call.apply(console.log, [console, "foo"]);
"#;
    let expected = r#"
Function.prototype.call.apply(console.log, [console, "foo"]);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
