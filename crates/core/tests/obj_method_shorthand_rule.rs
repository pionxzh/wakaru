mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ObjMethodShorthand;

fn apply(input: &str) -> String {
    render_rule(input, |_| ObjMethodShorthand)
}

#[test]
fn function_value_becomes_method_shorthand() {
    let input = r#"
const obj = {
    greet: function(name) {
        return "hello " + name;
    }
};
"#;
    let expected = r#"
const obj = {
    greet(name) {
        return "hello " + name;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multiple_methods_converted() {
    let input = r#"
const obj = {
    a: function() { return 1; },
    b: function(x) { return x * 2; }
};
"#;
    let expected = r#"
const obj = {
    a() { return 1; },
    b(x) { return x * 2; }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn anonymous_function_with_different_key_becomes_shorthand() {
    // When the function has no internal name, conversion is safe regardless of key name
    let input = r#"
const obj = {
    foo: function() {
        return 1;
    }
};
"#;
    let expected = r#"
const obj = {
    foo() {
        return 1;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_method_not_converted() {
    // Generator functions cannot be expressed as method shorthand without `*` —
    // keep as key-value pair to avoid changing semantics
    let input = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    let expected = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_function_expr_not_converted() {
    // A named function expression may reference itself by name inside the body.
    // Converting to shorthand would drop that internal name, breaking recursion.
    let input = r#"
const x = {foo: function foo() { return foo(); }};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn computed_key_not_converted() {
    // Computed property keys are dynamic — shorthand syntax does not support them
    let input = r#"
const x = {[foo]: function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn string_key_not_converted() {
    // String-keyed properties cannot use method shorthand syntax
    let input = r#"
const x = {"foo": function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn numeric_key_not_converted() {
    // Numeric-keyed properties cannot use method shorthand syntax
    let input = r#"
const x = {123: function() {}};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
