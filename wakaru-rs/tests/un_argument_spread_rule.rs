mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnArgumentSpread;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnArgumentSpread)
}

#[test]
fn converts_apply_with_undefined_to_spread() {
    let input = r#"
fn.apply(undefined, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_apply_with_null_to_spread() {
    let input = r#"
fn.apply(null, args);
"#;
    let expected = r#"
fn(...args);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_obj_method_apply_with_same_obj_to_spread() {
    let input = r#"
obj.fn.apply(obj, someArray);
"#;
    let expected = r#"
obj.fn(...someArray);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_convert_apply_with_different_this() {
    // obj.fn.apply(otherObj, ...) — not converted because thisArg != obj
    let input = r#"
fn.apply(obj, someArray);
"#;
    let expected = r#"
fn.apply(obj, someArray);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_convert_member_apply_with_null_this() {
    // obj.fn.apply(null, ...) — not converted because it changes `this` from
    // undefined to obj. The proper fix is namespace import decomposition
    // (obj.fn → fn), after which Pattern 1 handles it.
    let input = r#"
obj.fn.apply(null, someArray);
"#;
    let expected = r#"
obj.fn.apply(null, someArray);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_this_method_apply_with_this_to_spread() {
    let input = r#"
function foo() {
  this.fn.apply(this, someArray);
}
"#;
    let expected = r#"
function foo() {
  this.fn(...someArray);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn converts_obj_method_apply_with_array_expression() {
    let input = r#"
obj.fn.apply(obj, [1, 2, 3]);
"#;
    let expected = r#"
obj.fn(...[1, 2, 3]);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
