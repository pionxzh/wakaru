mod common;

use common::{assert_eq_normalized, render};

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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_function_expr_key_dropped() {
    let input = r#"
const obj = {
    foo: function bar() {
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generator_function_not_converted() {
    let input = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    // Generator should stay as key-value pair (not converted to shorthand)
    let expected = r#"
const obj = {
    gen: function* () {
        yield 1;
    }
};
"#;
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
