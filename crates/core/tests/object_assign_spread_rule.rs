mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ObjectAssignSpread;
use wakaru_core::RewriteLevel;

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
    // First arg is not a fresh object literal — mutates target, can't be spread.
    let input = r#"
Object.assign(target, source);
"#;
    let expected = r#"
Object.assign(target, source);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn safe_non_empty_literal_target() {
    let input = r#"
const x = Object.assign({ id: app_id }, app_info);
"#;
    let expected = r#"
const x = { id: app_id, ...app_info };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn unsafe_non_empty_literal_target_is_left_unchanged() {
    let input = r#"
const x = Object.assign({ set name(value) { record(value); } }, app_info);
const y = Object.assign({ __proto__: null, ready: true }, app_info);
"#;
    let expected = r#"
const x = Object.assign({ set name(value) { record(value); } }, app_info);
const y = Object.assign({ __proto__: null, ready: true }, app_info);
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

#[test]
fn keeps_object_assign_in_module_with_direct_eval() {
    let input = r#"
function f(a, s) {
    eval(s);
    return Object.assign({}, a);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_object_assign_in_module_with_with_statement() {
    let input = r#"
with (o) {
    x = Object.assign({}, a);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

fn apply_at(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |mark| {
        ObjectAssignSpread::new_with_level(mark, level)
    })
}

#[test]
fn below_aggressive_keeps_a_dynamic_source() {
    // `Object.assign` writes with [[Set]]: a source with an own `__proto__`
    // key (parsed JSON) sets the result's prototype, where spread would add an
    // own property. A dynamic source is a common hand-written merge.
    for input in [
        "const x = Object.assign({}, defaults, overrides);",
        "const x = Object.assign({}, state, { changes: next });",
        "const x = Object.assign({ a: 1 }, load());",
    ] {
        for level in [RewriteLevel::Minimal, RewriteLevel::Standard] {
            assert_eq_normalized(&apply_at(input, level), input);
        }
        assert!(
            !apply_at(input, RewriteLevel::Aggressive).contains("Object.assign"),
            "{input}"
        );
    }
}

#[test]
fn below_aggressive_converts_static_data_literal_sources() {
    let input = r#"const x = Object.assign({}, { a: 1, "b": 2, 3: c }, { d });"#;
    let expected = r#"const x = { a: 1, "b": 2, 3: c, d };"#;
    for level in [RewriteLevel::Minimal, RewriteLevel::Standard] {
        assert_eq_normalized(&apply_at(input, level), expected);
    }
}

#[test]
fn below_aggressive_keeps_literal_sources_that_may_carry_a_proto_key() {
    for input in [
        "const x = Object.assign({}, { [key]: 1 });",
        "const x = Object.assign({}, { ...rest });",
        r#"const x = Object.assign({}, { "__proto__": p });"#,
        "const x = Object.assign({}, { get a() { return 1; } });",
        "const x = Object.assign({}, { m() {} });",
    ] {
        assert_eq_normalized(&apply_at(input, RewriteLevel::Standard), input);
    }
}
