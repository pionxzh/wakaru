mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnDoubleNegation;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnDoubleNegation)
}

#[test]
fn strips_double_bang_in_if() {
    let input = "if (!!x) { a(); }";
    let expected = "if (x) { a(); }";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn strips_double_bang_in_while() {
    let input = "while (!!arr.length) { arr.pop(); }";
    let expected = "while (arr.length) { arr.pop(); }";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn strips_double_bang_in_do_while() {
    let input = "do { next(); } while (!!pending);";
    let expected = "do { next(); } while (pending);";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn strips_double_bang_in_for_test() {
    let input = "for (let i = 0; !!items[i]; i++) {}";
    let expected = "for(let i = 0; items[i]; i++){}";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn strips_double_bang_in_ternary_test() {
    let input = "const x = !!flag ? 'yes' : 'no';";
    let expected = "const x = flag ? 'yes' : 'no';";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn does_not_strip_in_assignment() {
    let input = "const x = !!flag;";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_strip_in_return() {
    let input = "function f() { return !!x; }";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_strip_in_logical_and() {
    let input = "const x = !!a && b;";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn does_not_strip_single_bang() {
    let input = "if (!x) { a(); }";
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn strips_nested_double_bang_in_if() {
    let input = "if (!!a.includes(b)) { ok(); }";
    let expected = "if (a.includes(b)) { ok(); }";
    assert_eq_normalized(&apply(input), expected);
}
