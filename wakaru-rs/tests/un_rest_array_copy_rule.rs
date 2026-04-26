mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_rs::rules::UnRestArrayCopy;

// ── nested / name-collision cases ────────────────────────────────────────────
//
// ArgRest always names the rest parameter `args` with ctxt = SyntaxContext::empty().
// When two nested functions both receive `...args`, they share the same (sym, ctxt).
// If the outer's copy variable is referenced inside the inner function (closure
// capture), replacing it with `args` would silently pick up the inner binding.
// The rule detects this and skips the outer transformation to preserve semantics.

fn apply(input: &str) -> String {
    render_rule(input, |_| UnRestArrayCopy)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn removes_babel_rest_copy_loop() {
    // Typical Babel ES5 output for a rest-param function; copy var name (`i`) is reused
    let input = r#"
export function t() {
    for (var o = arguments.length, i = Array(o), a = 0; a < o; a++) i[a] = arguments[a];
    return i[0] + i[1];
}
"#;
    let expected = r#"
export function t(...i) {
    return i[0] + i[1];
}
"#;
    assert_eq_normalized(&apply_pipeline(input), expected);
}

#[test]
fn removes_babel_rest_copy_loop_with_block_body() {
    // Babel sometimes wraps the body in braces (UnCurlyBraces will add them too)
    let input = r#"
export function t() {
    for (var o = arguments.length, i = Array(o), a = 0; a < o; a++) {
        i[a] = arguments[a];
    }
    return i.join(", ");
}
"#;
    let expected = r#"
export function t(...i) {
    return i.join(", ");
}
"#;
    assert_eq_normalized(&apply_pipeline(input), expected);
}

#[test]
fn removes_babel_rest_copy_loop_new_array_variant() {
    // Some Babel versions emit `new Array(n)` instead of `Array(n)`
    let input = r#"
export function t() {
    for (var n = arguments.length, r = new Array(n), o = 0; o < n; o++) {
        r[o] = arguments[o];
    }
    foo(r);
}
"#;
    let expected = r#"
export function t(...r) {
    foo(r);
}
"#;
    assert_eq_normalized(&apply_pipeline(input), expected);
}

#[test]
fn copy_loop_without_arguments_not_removed() {
    // The source of the copy is NOT the rest param — do not transform
    let input = r#"
function t(...args) {
    for (let o = other.length, i = Array(o), a = 0; a < o; a++) {
        i[a] = other[a];
    }
    return i;
}
"#;
    let output = apply(input);
    // `other` is not a rest param — loop should remain
    assert!(output.contains("for"), "loop was wrongly removed: {output}");
}

#[test]
fn no_rest_param_no_transform() {
    // No rest parameter at all — should not trigger
    let input = r#"
function t(a, b) {
    return a + b;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn nested_function_copy_loop_removed_independently() {
    // Named function declarations are not converted to arrows, but the copy loop is
    // still removed and the rest param added correctly.
    let input = r#"
export function outer() {
    function inner() {
        for (var o = arguments.length, i = Array(o), a = 0; a < o; a++) i[a] = arguments[a];
        return i[0];
    }
    return inner;
}
"#;
    let expected = r#"
export function outer() {
    function inner(...i) {
        return i[0];
    }
    return inner;
}
"#;
    assert_eq_normalized(&apply_pipeline(input), expected);
}
