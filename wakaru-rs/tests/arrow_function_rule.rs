mod common;

use common::{assert_eq_normalized, render};

#[test]
fn single_return_becomes_arrow_expression() {
    let input = r#"
const double = [1, 2, 3].map(function(x) { return x * 2; });
"#;
    let expected = r#"
const double = [1, 2, 3].map(x => x * 2);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_statement_body_keeps_block() {
    let input = r#"
arr.forEach(function(x) {
    console.log(x);
    doSomething(x);
});
"#;
    let expected = r#"
arr.forEach(x => {
    console.log(x);
    doSomething(x);
});
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_this_not_converted() {
    let input = r#"
const obj = { fn: function() { return this.x; } };
"#;
    // Functions using 'this' should NOT be converted to arrows
    let expected = r#"
const obj = { fn() { return this.x; } };
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_arguments_not_converted() {
    let input = r#"
const fn = function() { return arguments[0]; };
"#;
    // Functions using 'arguments' should NOT be converted to arrows
    let expected = r#"
const fn = function() { return arguments[0]; };
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_not_converted() {
    let input = r#"
const gen = function* () { yield 1; };
"#;
    let expected = r#"
const gen = function* () { yield 1; };
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn zero_params_arrow() {
    let input = r#"
const fn = function() { return 42; };
"#;
    let expected = r#"
const fn = () => 42;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_params_arrow() {
    let input = r#"
const add = function(a, b) { return a + b; };
"#;
    let expected = r#"
const add = (a, b) => a + b;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
