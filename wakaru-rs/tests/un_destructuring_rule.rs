mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnDestructuring;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnDestructuring)
}

#[test]
fn reconstructs_array_rest_from_ref_slice() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_array_rest_with_holes() {
    let input = r#"
var _ref = arr;
var first = _ref[0];
var third = _ref[2];
var rest = _ref.slice(3);
"#;
    let expected = r#"
var [first, , third, ...rest] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_array_rest_when_slice_has_end_arg() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1, 3);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn rejects_array_rest_when_later_index_is_inside_rest() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
var tail = _ref.slice(1);
var third = _ref[2];
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_array_default_from_temp_conditional() {
    let input = r#"
var _ref = arr;
var _tmp = _ref[0];
var head = _tmp === void 0 ? "default" : _tmp;
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head = "default", ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_from_temp_conditional() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var foo = _tmp === void 0 ? 1 : _tmp;
var bar = _ref.bar;
"#;
    let expected = r#"
var { foo = 1, bar } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_alias_default_from_temp_conditional() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.foo;
var value = _tmp === void 0 ? 1 : _tmp;
var label = _ref.label;
"#;
    let expected = r#"
var { foo: value = 1, label } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_false_from_temp_logical_and() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.exact;
var exact = _tmp !== undefined && _tmp;
var strict = _ref.strict;
"#;
    let expected = r#"
var { exact = false, strict } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_default_true_from_temp_logical_or() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.pure;
var pure = _tmp === undefined || _tmp;
var mode = _ref.mode;
"#;
    let expected = r#"
var { pure = true, mode } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn reconstructs_object_alias_default_false_from_reversed_undefined_check() {
    let input = r#"
var _ref = opts;
var _tmp = _ref.exact;
var enabled = undefined !== _tmp && _tmp;
var strict = _ref.strict;
"#;
    let expected = r#"
var { exact: enabled = false, strict } = opts;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn rejects_group_when_ref_is_used_later() {
    let input = r#"
var _ref = arr;
var head = _ref[0];
consume(_ref);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_plain_index_groups_to_smart_inline() {
    let input = r#"
var _ref = arr;
var first = _ref[0];
var second = _ref[1];
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reconstructs_rest_after_spread_array_unwrap() {
    let input = r#"
var _ref = [...arr];
var head = _ref[0];
var tail = _ref.slice(1);
"#;
    let expected = r#"
var [head, ...tail] = arr;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn leaves_direct_loose_array_rest() {
    let input = r#"
const head = values[0];
const rest = values.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_direct_loose_array_rest_after_multiple_indexes() {
    let input = r#"
const head = ref[0];
const neck = ref[1];
const tail = ref.slice(2);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn rejects_direct_loose_array_rest_when_explicit_index_overlaps_rest() {
    let input = r#"
const head = ref[0];
const neck = ref[1];
const tail = ref.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn leaves_direct_slice_without_index_access() {
    let input = r#"
const rest = values.slice(1);
"#;
    assert_eq_normalized(&apply(input), input);
}
