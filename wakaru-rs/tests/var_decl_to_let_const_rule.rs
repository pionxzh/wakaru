mod common;

use common::{assert_eq_normalized, render};

#[test]
fn var_never_reassigned_becomes_const() {
    let input = r#"
var x = 1;
"#;
    let expected = r#"
const x = 1;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_reassigned_becomes_let() {
    let input = r#"
var x = 1;
x = 2;
"#;
    let expected = r#"
let x = 1;
x = 2;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_updated_becomes_let() {
    let input = r#"
var i = 0;
i++;
"#;
    let expected = r#"
let i = 0;
i++;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_without_init_becomes_let() {
    let input = r#"
var x;
x = 10;
"#;
    let expected = r#"
let x;
x = 10;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_inside_function_scope() {
    let input = r#"
function foo() {
    var a = 1;
    var b = 2;
    b = 3;
    return a + b;
}
"#;
    let expected = r#"
function foo() {
    const a = 1;
    let b = 2;
    b = 3;
    return a + b;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_assigned_in_nested_closure_becomes_let() {
    let input = r#"
var counter = 0;
function inc() {
    counter++;
}
"#;
    let expected = r#"
let counter = 0;
function inc() {
    counter++;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
