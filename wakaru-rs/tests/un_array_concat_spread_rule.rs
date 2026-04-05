mod common;
use common::{assert_eq_normalized, render};

#[test]
fn simplifies_literal_array_concat_single_element() {
    let input = r#"
const x = [a].concat(b);
"#;
    let expected = r#"
const x = [a, ...b];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_literal_array_concat_multiple_elements() {
    let input = r#"
const x = [a, b].concat(c);
"#;
    let expected = r#"
const x = [a, b, ...c];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_concat_with_multiple_args() {
    let input = r#"
const x = [a].concat(b, c);
"#;
    let expected = r#"
const x = [a, ...b, ...c];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_concat_with_array_literal_arg() {
    let input = r#"
const x = [a].concat([b, c]);
"#;
    let expected = r#"
const x = [a, b, c];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_empty_array_concat() {
    let input = r#"
const x = [].concat(a);
"#;
    let expected = r#"
const x = [...a];
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn simplifies_spread_over_concat_pattern() {
    // The Babel class constructor pattern: e.call(...[this].concat(args))
    // After concat→spread: e.call(...[this, ...args])
    // The spread-over-array inlining (...[a, ...b] → a, ...b) is handled
    // by UnArgumentSpread, so we just verify the concat is simplified.
    let input = r#"
const x = e.call(...[this].concat(args));
"#;
    let output = render(input);
    assert!(!output.contains(".concat("), "concat should be eliminated");
    assert!(output.contains("...args"), "spread should be preserved");
}

#[test]
fn preserves_variable_concat() {
    // Don't transform x.concat(y) where x is not an array literal
    let input = r#"
const x = arr.concat(other);
"#;
    let output = render(input);
    assert!(output.contains(".concat("), "should not transform variable.concat()");
}
