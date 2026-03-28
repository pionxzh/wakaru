mod common;

use common::{assert_eq_normalized, render};

#[test]
fn arguments_index_becomes_rest_args() {
    let input = r#"
function foo() {
    return arguments[0] + arguments[1];
}
"#;
    let expected = r#"
function foo(...args) {
    return args[0] + args[1];
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn arguments_length_becomes_rest_length() {
    let input = r#"
function foo() {
    return arguments.length;
}
"#;
    let expected = r#"
function foo(...args) {
    return args.length;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn arguments_loop_pattern() {
    let input = r#"
function sum() {
    var total = 0;
    for (var i = 0; i < arguments.length; i++) {
        total += arguments[i];
    }
    return total;
}
"#;
    let expected = r#"
function sum(...args) {
    let total = 0;
    for (let i = 0; i < args.length; i++) {
        total += args[i];
    }
    return total;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn arguments_with_variable_index() {
    let input = r#"
function get(i) {
    return arguments[i];
}
"#;
    // Function has a formal param — should not transform
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_params_not_converted() {
    // Formal params are present — cannot safely add ...args
    let input = r#"
function foo(a, b) {
    return arguments[0];
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_already_has_rest_not_converted() {
    let input = r#"
function foo(...rest) {
    return rest[0];
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn bare_arguments_reference_not_converted() {
    // Passing `arguments` as a whole value is unsafe to transform
    let input = r#"
function foo() {
    return bar(arguments);
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn arguments_spread_not_converted() {
    let input = r#"
function foo() {
    return bar(...arguments);
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn nested_function_arguments_not_conflated() {
    // Inner function's `arguments` should be transformed independently;
    // outer function has no `arguments` so it is left alone.
    let input = r#"
function outer() {
    function inner() {
        return arguments[0];
    }
    return inner;
}
"#;
    let expected = r#"
function outer() {
    function inner(...args) {
        return args[0];
    }
    return inner;
}
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_arguments_usage_not_converted() {
    let input = r#"
function foo() {
    return 42;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}
