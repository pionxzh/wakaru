mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::{rules::UnParameters, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |unresolved_mark| {
        UnParameters::new(unresolved_mark, level)
    })
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
fn shadowed_undefined_guard_stays_in_body() {
    let input = r#"
function foo(a) {
  var undefined = 42;
  if (a === undefined) a = 1;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn duplicate_param_void0_guard_stays_in_body() {
    let input = r#"
function foo(a, a) {
  if (a === void 0) a = 1;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
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

#[test]
fn default_guard_referencing_later_param_stays_in_body() {
    let input = r#"
function foo(a, b) {
  if (a === void 0) a = b.value;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_default_guard_referencing_later_param_stays_in_body() {
    let input = r#"
const foo = (a, b) => {
  if (a === void 0) a = b.value;
  return a;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn default_guard_referencing_same_param_stays_in_body() {
    let input = r#"
function foo(a) {
  if (a === void 0) a = a;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn default_guard_referencing_body_var_stays_in_body() {
    let input = r#"
function foo(a) {
  if (a === void 0) a = x;
  var x = 1;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn default_guard_referencing_earlier_body_var_stays_in_body() {
    let input = r#"
function foo(a) {
  var x = 1;
  if (a === void 0) a = x;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_default_guard_referencing_body_var_stays_in_body() {
    let input = r#"
const foo = (a) => {
  if (a === void 0) a = x;
  var x = 1;
  return a;
};
"#;
    assert_eq_normalized(&apply(input), input);
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
fn shadowed_arguments_param_stays_in_body() {
    let input = r#"
function foo(arguments) {
  var b = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 1;
  return b;
}
"#;
    assert_eq_normalized(&apply(input), input);
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
fn optional_arguments_alias_becomes_plain_param() {
    let input = r#"
function foo(a) {
  let b = arguments.length > 1 ? arguments[1] : undefined;
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
fn minimal_does_not_recover_optional_arguments_alias() {
    let input = r#"
function foo(a) {
  let b = arguments.length > 1 ? arguments[1] : undefined;
  return b;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn arguments_alias_to_existing_param_name_stays_in_body() {
    let input = r#"
function foo(a) {
  const b = arguments[0];
  return b;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_default_to_existing_param_name_stays_in_body() {
    let input = r#"
function foo(a) {
  var b = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : 1;
  return b;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_default_referencing_later_param_stays_in_body() {
    let input = r#"
function foo(q, K, _, z) {
  var q = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : _.bits || 2048;
  var K = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : _.e || 65537;
  return [q, K, _, z];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_default_referencing_same_param_stays_in_body() {
    let input = r#"
function foo(a) {
  var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : a;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arguments_default_referencing_body_var_stays_in_body() {
    let input = r#"
function foo(a) {
  var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : x;
  var x = 1;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn inline_arguments_default_referencing_body_var_stays_in_body() {
    let input = r#"
function foo() {
  return arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : x;
  var x = 1;
}
"#;
    assert_eq_normalized(&apply(input), input);
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
fn duplicate_param_object_alias_default_stays_in_body() {
    let input = r#"
function foo(options, options) {
  const opts = options === undefined ? {} : options;
  return opts.name;
}
"#;
    assert_eq_normalized(&apply(input), input);
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
fn object_destructuring_alias_becomes_param_pattern() {
    let input = r#"
function foo(_ref) {
  const { name, age = fallback } = _ref;
  return name + age;
}
"#;
    let expected = r#"
function foo({ name, age = fallback }) {
  return name + age;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_destructuring_alias_default_becomes_param_pattern_default() {
    let input = r#"
function foo(_ref) {
  const { name = fallback } = _ref === undefined ? {} : _ref;
  return name;
}
"#;
    let expected = r#"
function foo({ name = fallback } = {}) {
  return name;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_object_destructuring_alias_becomes_param_pattern() {
    let input = r#"
function nested(_ref = {}) {
  const { outer: { value = fallbackValue } = {} } = _ref;
  return value;
}
"#;
    let expected = r#"
function nested({ outer: { value = fallbackValue } = {} } = {}) {
  return value;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn array_destructuring_alias_becomes_param_pattern() {
    let input = r#"
function foo(_ref = []) {
  const [first, second = fallback] = _ref;
  return first + second;
}
"#;
    let expected = r#"
function foo([first, second = fallback] = []) {
  return first + second;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn array_destructuring_conditional_alias_becomes_param_pattern_default() {
    let input = r#"
function foo(_ref) {
  const [first, second = fallback] = _ref === undefined ? [] : _ref;
  return first + second;
}
"#;
    let expected = r#"
function foo([first, second = fallback] = []) {
  return first + second;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arrow_object_destructuring_alias_becomes_param_pattern() {
    let input = r#"
const foo = (_ref) => {
  const { name } = _ref;
  return name;
};
"#;
    let expected = r#"
const foo = ({ name }) => {
  return name;
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn minimal_does_not_fold_destructured_param_alias() {
    let input = r#"
function foo(_ref) {
  const { name } = _ref;
  return name;
}
"#;
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn destructured_param_alias_used_later_stays_in_body() {
    let input = r#"
function foo(_ref) {
  const { name } = _ref;
  return _ref.name || name;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructured_param_alias_used_in_default_stays_in_body() {
    let input = r#"
function foo(_ref) {
  const { name = _ref.name } = _ref;
  return name;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructured_param_collision_stays_in_body() {
    let input = r#"
function foo(name, _ref) {
  const { name } = _ref;
  return name;
}
"#;
    assert_eq_normalized(&apply(input), input);
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
