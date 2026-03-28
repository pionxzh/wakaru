mod common;

use wakaru_rs::rules::ArrowFunction;
use common::{assert_eq_normalized, render_pipeline, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| ArrowFunction)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn single_return_becomes_arrow_expression() {
    let input = r#"
const double = [1, 2, 3].map(function(x) { return x * 2; });
"#;
    let expected = r#"
const double = [1, 2, 3].map(x => x * 2);
"#;
    let output = apply(input);
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
    let output = apply(input);
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
    let output = apply(input);
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
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_this_not_converted() {
    // `this` binding is different in arrow functions — must not convert
    let input = r#"
const obj = { fn: function() { return this.x; } };
"#;
    let expected = r#"
const obj = { fn: function() { return this.x; } };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn function_with_arguments_converted_via_arg_rest() {
    // ArgRest rewrites arguments[N] → args[N] first, then ArrowFunction can convert.
    // Arrow functions have no own `arguments`, but after ArgRest runs that is no
    // longer a blocker.
    let input = r#"
const fn = function() { return arguments[0]; };
"#;
    let expected = r#"
const fn = (...args) => args[0];
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_not_converted() {
    // Arrow functions cannot be generators
    let input = r#"
const gen = function* () { yield 1; };
"#;
    let expected = r#"
const gen = function* () { yield 1; };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_function_expr_not_converted() {
    // Named function expressions may reference themselves by name — converting
    // to an arrow would break that self-reference
    let input = r#"
f = function fact(n) { return n * fact(n - 1); };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn object_method_value_not_converted_to_arrow() {
    // Object method values may use `this`; the obj-method shorthand rule handles
    // them separately. Arrow conversion must not fire here.
    let input = r#"
({foo: function() {}});
"#;
    let output = apply(input);
    assert!(!output.contains("=>"), "object method became arrow: {output}");
}

#[test]
fn bind_this_converted_to_arrow() {
    // `fn.bind(this)` explicitly locks `this`, making the function semantically
    // equivalent to an arrow — safe to convert
    let input = r#"
a(function(x) { this.x = x; }.bind(this));
"#;
    let expected = r#"
a(x => { this.x = x; });
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn async_anonymous_function_converted() {
    // Async anonymous function expressions without `this`/`arguments` can safely
    // become async arrow functions
    let input = r#"
f = async function() { return 1; };
"#;
    let expected = r#"
f = async () => 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}


