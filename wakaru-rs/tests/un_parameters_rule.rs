mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::{rules::UnParameters, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnParameters::new(level))
}

// --- void 0 / undefined guard patterns ---

#[test]
fn void0_guard_becomes_default_param() {
    let input = r#"
function foo(a, b) {
  if (a === void 0) { a = 1; }
  if (b === void 0) b = 2;
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn void0_guard_reversed_operands() {
    let input = r#"
function foo(a, b) {
  if (void 0 === a) a = 1;
  if (void 0 === b) { b = 2; }
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn undefined_guard_becomes_default_param() {
    let input = r#"
function foo(a, b) {
  if (a === undefined) a = 1;
  if (undefined === b) b = 2;
  return a + b;
}
"#;
    let expected = r#"
function foo(a = 1, b = 2) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn void0_guard_in_arrow_function() {
    let input = r#"
const test = (a, b) => {
  if (a === void 0) a = 1;
  if (void 0 === b) b = 2;
};
"#;
    let expected = r#"
const test = (a = 1, b = 2) => {};
"#;
    assert_eq_normalized(&apply(input), expected);
}

// --- falsy-coalescing patterns are intentionally not rewritten ---

#[test]
fn or_assignment_stays_as_is() {
    let input = r#"
function foo(a) {
    a = a || 2;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn or_assignment_multiple_params_stay_as_is() {
    let input = r#"
function foo(a, b, c) {
    a = a || "hello";
    b = b || {};
    c = c || [];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

// --- ternary-assignment pattern ---

#[test]
fn ternary_self_check_stays_as_is() {
    let input = r#"
function foo(a) {
    a = a ? a : 4;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_or_assignment_with_zero_not_converted() {
    let input = r#"
function foo(a) {
    a = a || 2;
    return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn known_bug_ternary_self_check_with_empty_string_not_converted() {
    let input = r#"
function foo(a) {
    a = a ? a : "fallback";
    return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

// --- no-op cases ---

#[test]
fn noop_no_guard() {
    let input = r#"
function foo(a, b) {
  return a + b;
}
"#;
    let expected = r#"
function foo(a, b) {
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn noop_guard_for_non_param() {
    // `c` is not in the parameter list — guard should be left untouched
    let input = r#"
function foo(a, b) {
  if (c === void 0) c = 1;
  return a + b;
}
"#;
    let expected = r#"
function foo(a, b) {
  if (c === void 0) c = 1;
  return a + b;
}
    "#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_boolean_presence_default_becomes_param_default() {
    let input = r#"
function foo(a) {
  var b = !(arguments.length > 1 && arguments[1] !== undefined) || arguments[1];
  return b;
}
"#;
    let expected = r#"
function foo(a, b = true) {
  return b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_recover_arguments_boolean_presence_default() {
    let input = r#"
function foo(a) {
  var b = !(arguments.length > 1 && arguments[1] !== undefined) || arguments[1];
  return b;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn arguments_inline_default_expression_becomes_param_default() {
    let input = r#"
function foo(a, b) {
  return bar(a, b, arguments.length > 2 && arguments[2] !== undefined ? arguments[2] : null);
}
"#;
    let expected = r#"
function foo(a, b, _param_2 = null) {
  return bar(a, b, _param_2);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_recover_inline_arguments_default_expression() {
    let input = r#"
function foo(a, b) {
  return bar(a, b, arguments.length > 2 && arguments[2] !== undefined ? arguments[2] : null);
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn arguments_alias_becomes_plain_param() {
    let input = r#"
function foo(a) {
  const b = arguments[1];
  return b;
}
"#;
    let expected = r#"
function foo(a, b) {
  return b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn undefined_object_alias_becomes_default_param() {
    let input = r#"
function foo(options) {
  const opts = options === undefined ? {} : options;
  return opts.name;
}
"#;
    let expected = r#"
function foo(options = {}) {
  const opts = options;
  return opts.name;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_recover_undefined_object_alias_default() {
    let input = r#"
function foo(options) {
  const opts = options === undefined ? {} : options;
  return opts.name;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn undefined_object_destructuring_keeps_body_defaults() {
    let input = r#"
function foo(options) {
  const { name = fallback } = options === undefined ? {} : options;
  const fallback = "x";
  return name;
}
"#;
    let expected = r#"
function foo(options = {}) {
  const { name = fallback } = options;
  const fallback = "x";
  return name;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn undefined_object_alias_arrow_becomes_default_param() {
    let input = r#"
const foo = (options) => {
  const opts = options === undefined ? {} : options;
  return opts.name;
};
"#;
    let expected = r#"
const foo = (options = {}) => {
  const opts = options;
  return opts.name;
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_recover_undefined_object_alias_default_in_arrow() {
    let input = r#"
const foo = (options) => {
  const opts = options === undefined ? {} : options;
  return opts.name;
};
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn non_empty_object_default_stays_in_body() {
    let input = r#"
function foo(options) {
  const opts = options === undefined ? makeOptions() : options;
  return opts.name;
}
"#;
    assert_eq_normalized(&apply(input), input);
}
