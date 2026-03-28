mod common;

use common::{assert_eq_normalized, render};

#[test]
fn inline_single_use_temp_var() {
    let input = r#"
const t = foo;
bar(t);
"#;
    let expected = r#"
bar(foo);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_multi_use_temp_var() {
    // t is used twice — should NOT be inlined
    let input = r#"
const t = foo;
bar(t);
baz(t);
"#;
    let expected = r#"
const t = foo;
bar(t);
baz(t);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_literal_const() {
    // Literal constants are intentionally named — keep them
    let input = r#"
const n = 42;
process(n);
"#;
    let expected = r#"
const n = 42;
process(n);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_into_nested_function() {
    // t used inside nested fn — top-level count is 0, shouldn't inline
    // ArrowFunction rule converts the function expression to an arrow function.
    let input = r#"
const t = foo;
const fn2 = function() { return t; };
"#;
    let expected = r#"
const t = foo;
const fn2 = () => t;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_property_destructuring() {
    // aliases a/b/c are ≤2 chars → SmartRename converts them to shorthand x/y/z
    let input = r#"
const a = obj.x;
const b = obj.y;
const c = obj.z;
"#;
    let expected = r#"
const { x, y, z } = obj;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_array_destructuring() {
    let input = r#"
const a = arr[0];
const b = arr[1];
const c = arr[2];
"#;
    let expected = r#"
const [a, b, c] = arr;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_array_destructuring_with_holes() {
    let input = r#"
const a = arr[0];
const c = arr[2];
"#;
    let expected = r#"
const [a, , c] = arr;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_group_single_property_access() {
    // Only one access — not worth destructuring
    let input = r#"
const a = obj.x;
"#;
    let expected = r#"
const a = obj.x;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_group_single_index_access() {
    let input = r#"
const a = arr[0];
"#;
    let expected = r#"
const a = arr[0];
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_property_shorthand_after_rename() {
    // When alias == prop key name, smart-rename converts to shorthand
    let input = r#"
const x = obj.x;
const y = obj.y;
"#;
    let expected = r#"
const { x, y } = obj;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

// ============================================================
// Zero-param arrow wrapper inlining (require.n / webpack4 pattern)
// ============================================================

#[test]
fn inline_arrow_wrapper_into_nested_function() {
    // `const o = () => r` used once inside a nested function should be inlined
    // globally across the function boundary, and the resulting (() => r)() call
    // should collapse to just `r` via the second UnIife pass.
    let input = r#"
const o = () => r;
function foo() {
    return o();
}
"#;
    let expected = r#"
function foo() {
    return r;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_arrow_wrapper_at_all_use_sites() {
    // Arrow wrappers are inlined everywhere regardless of use count —
    // they are pure aliases with no semantic value (e.g. require.n wrappers).
    let input = r#"
const o = () => r;
function foo() { return o(); }
function bar() { return o(); }
"#;
    let expected = r#"
function foo() { return r; }
function bar() { return r; }
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn arrow_wrapper_dot_a_accessor_stays_as_is() {
    let input = r#"
const o = () => r;
function foo() {
    return o.a;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn known_bug_arrow_wrapper_dot_a_non_webpack_shape_not_inlined() {
    let input = r#"
const o = () => r;
console.log(o.a);
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn grouped_object_access_preserves_binding_context_for_followup_renames() {
    let input = r#"
const i = Object.defineProperty;
const c = Object.getPrototypeOf;
const s = c && c(Object);
i(target, key, desc);
"#;
    let expected = r#"
const { defineProperty, getPrototypeOf } = Object;
const s = getPrototypeOf && getPrototypeOf(Object);
defineProperty(target, key, desc);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
