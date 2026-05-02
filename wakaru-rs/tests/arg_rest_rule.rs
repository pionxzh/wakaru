mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::{rules::ArgRest, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| ArgRest::new(level))
}

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
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_convert_arguments_index_to_rest_args() {
    let input = r#"
function foo() {
    return arguments[0] + arguments[1];
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
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
    assert_eq_normalized(&apply(input), expected);
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
    var total = 0;
    for (var i = 0; i < args.length; i++) {
        total += args[i];
    }
    return total;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_with_variable_index() {
    let input = r#"
function get(i) {
    return arguments[i];
}
"#;
    // Function has a formal param — should not transform
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_params_not_converted() {
    // Accessing the fixed-parameter prefix through `arguments` is still unsafe.
    let input = r#"
function foo(a, b) {
    return arguments[0];
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_with_fixed_params_tail_indices_becomes_rest_args() {
    let input = r#"
function foo(a, b) {
    return arguments[2] + arguments[3];
}
"#;
    let expected = r#"
function foo(a, b, ...args) {
    return args[0] + args[1];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_already_has_rest_not_converted() {
    let input = r#"
function foo(...rest) {
    return rest[0];
}
"#;
    let output = apply(input);
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
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn arguments_spread_not_converted() {
    let input = r#"
function foo() {
    return bar(...arguments);
}
"#;
    let output = apply(input);
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
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn no_arguments_usage_not_converted() {
    let input = r#"
function foo() {
    return 42;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// ---------------------------------------------------------------------------
// Class constructor support
// ---------------------------------------------------------------------------

#[test]
fn constructor_arguments_becomes_rest_param() {
    // ArgRest must also visit Constructor nodes, not just Function nodes
    let input = r#"
class Foo {
    constructor() {
        console.log(arguments[0]);
    }
}
"#;
    let expected = r#"
class Foo {
    constructor(...args) {
        console.log(args[0]);
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn constructor_babel_copy_loop_removed() {
    // The Babel rest-args copy loop should be removed when rest param is added
    let input = r#"
class Foo {
    constructor() {
        for (var o = arguments.length, i = Array(o), a = 0; a < o; a++) {
            i[a] = arguments[a];
        }
        this.items = i;
    }
}
"#;
    let expected = r#"
class Foo {
    constructor(...i) {
        this.items = i;
    }
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

// ---------------------------------------------------------------------------
// Copy loop removal in regular functions
// ---------------------------------------------------------------------------

#[test]
fn function_babel_copy_loop_removed() {
    let input = r#"
function foo() {
    for (var len = arguments.length, args = Array(len), i = 0; i < len; i++) {
        args[i] = arguments[i];
    }
    return args;
}
"#;
    let expected = r#"
function foo(...args) {
    return args;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn function_babel_tail_copy_loop_removed() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i - 2] = arguments[i];
    }
    return bar(a, b, rest);
}
"#;
    let expected = r#"
function foo(a, b, ...rest) {
    return bar(a, b, rest);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn tail_copy_loop_with_wrong_test_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i <= len; i++) {
        rest[i - 2] = arguments[i];
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn tail_copy_loop_with_wrong_write_index_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i] = arguments[i];
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn tail_copy_loop_with_extra_body_statement_is_preserved() {
    let input = r#"
function foo(a, b) {
    for (var len = arguments.length, rest = Array(len > 2 ? len - 2 : 0), i = 2; i < len; i++) {
        rest[i - 2] = arguments[i];
        observe(arguments.callee);
    }
    return rest;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn copy_loop_preserved_when_not_arguments_pattern() {
    // A for loop that doesn't match the Babel copy pattern should be kept
    let input = r#"
function foo() {
    for (var i = 0; i < 10; i++) {
        console.log(arguments[i]);
    }
}
"#;
    let output = apply(input);
    // The for loop should still be present (it's not the copy pattern)
    assert!(output.contains("for"), "non-copy for loop should be preserved: {}", output);
}
