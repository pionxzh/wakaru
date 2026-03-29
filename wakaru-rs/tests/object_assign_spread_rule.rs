mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::ObjectAssignSpread;

fn apply(input: &str) -> String {
    render_rule(input, ObjectAssignSpread::new)
}

#[test]
fn empty_target_single_source() {
    let input = r#"
const x = Object.assign({}, defaults);
"#;
    let expected = r#"
const x = { ...defaults };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn empty_target_multiple_sources() {
    let input = r#"
const x = Object.assign({}, a, b, c);
"#;
    let expected = r#"
const x = { ...a, ...b, ...c };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_object_literal_source() {
    let input = r#"
const x = Object.assign({}, { a: 1, b: 2 });
"#;
    let expected = r#"
const x = { a: 1, b: 2 };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mix_of_spread_and_inline() {
    let input = r#"
const x = Object.assign({}, base, { extra: 1 }, more);
"#;
    let expected = r#"
const x = { ...base, extra: 1, ...more };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn non_empty_first_arg_is_left_unchanged() {
    // First arg is not `{}` — mutates target, can't be spread.
    let input = r#"
Object.assign(target, source);
Object.assign({ a: 1 }, source);
"#;
    let expected = r#"
Object.assign(target, source);
Object.assign({ a: 1 }, source);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_args_is_left_unchanged() {
    let input = r#"
const x = Object.assign({});
"#;
    let expected = r#"
const x = {};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_object_assign() {
    let input = r#"
const x = Object.assign({}, Object.assign({}, a, b), c);
"#;
    let expected = r#"
const x = { ...a, ...b, ...c };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn accessor_literal_source_stays_spread() {
    let input = r#"
const x = Object.assign({}, { get value() { return compute(); } });
"#;
    let expected = r#"
const x = { ...{ get value() { return compute(); } } };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn method_literal_source_stays_spread() {
    let input = r#"
const x = Object.assign({}, { render() { return view; } });
"#;
    let expected = r#"
const x = { ...{ render() { return view; } } };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bare_proto_literal_source_stays_spread() {
    let input = r#"
const x = Object.assign({}, { __proto__: null, ready: true });
"#;
    let expected = r#"
const x = { ...{ __proto__: null, ready: true } };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn shadowed_object_assign_is_left_unchanged() {
    let input = r#"
function build(Object) {
  return Object.assign({}, defaults);
}
"#;
    let expected = r#"
function build(Object) {
  return Object.assign({}, defaults);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
