mod common;
use common::{assert_eq_normalized, render};

#[test]
fn inlines_spread_array_literal_in_call() {
    let input = r#"
const x = fn(...[a, b, c]);
"#;
    let expected = r#"
const x = fn(a, b, c);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn inlines_spread_array_with_inner_spread() {
    let input = r#"
const x = fn(...[a, ...b]);
"#;
    let expected = r#"
const x = fn(a, ...b);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_class_constructor_pattern() {
    // e.call(...[this, ...args]) → e.call(this, ...args)
    let input = r#"
const x = e.call(...[this, ...args]);
"#;
    let expected = r#"
const x = e.call(this, ...args);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_spread_over_non_literal() {
    let input = r#"
const x = fn(...arr);
"#;
    let output = render(input);
    assert!(output.contains("...arr"), "should preserve spread over non-literal");
}

#[test]
fn inlines_mixed_args() {
    // Mix of regular args and spread array literal
    let input = r#"
const x = fn(a, ...[b, c], d);
"#;
    let expected = r#"
const x = fn(a, b, c, d);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn inlines_empty_spread_array() {
    let input = r#"
const x = fn(a, ...[], b);
"#;
    let expected = r#"
const x = fn(a, b);
"#;
    assert_eq_normalized(&render(input), expected);
}
