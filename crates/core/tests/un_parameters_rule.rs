mod common;

use common::{assert_eq_normalized, render_rule};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{BindingIdent, Decl, EsVersion, Function, ModuleItem, Pat, Stmt};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};
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
fn direct_strict_directive_preserves_simple_parameters() {
    let input = r#"
function foo(a) {
  "use asm";
  "use strict";
  if (a === void 0) a = 1;
  return a;
}
"#;

    assert_eq_normalized(&apply(input), input);
}

#[test]
fn parameter_recovery_does_not_promote_later_strict_string_to_directive() {
    let input = r#"
function foo(a) {
  if (a === void 0) a = 1;
  "use strict";
  return a;
}
const bar = (a) => {
  if (a === void 0) a = 2;
  "use strict";
  return a;
};
"#;

    assert_eq_normalized(&apply(input), input);
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

/// A minifier can reuse a parameter's name for a later argument once the
/// first value is dead: the body `var` redeclares the parameter, so it is the
/// same binding. Moving it into slot 1 would declare `e` twice, a
/// SyntaxError beside a default. Shape wild-observed (Babel's
/// `arguments`-based default after a destructured first parameter).
#[test]
fn arguments_default_var_redeclaring_another_param_stays_in_body() {
    let input = r#"
function foo(e) {
  var n = e.items;
  var e = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : fallback;
  return e(n);
}
"#;
    let expected = r#"
function foo(e, _param_1 = fallback) {
  var n = e.items;
  var e = _param_1;
  return e(n);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_alias_var_redeclaring_another_param_stays_in_body() {
    let input = r#"
function foo(e) {
  var n = e.items;
  var e = arguments[1];
  return e(n);
}
"#;
    let output = apply(input);
    assert!(
        !output.contains("function foo(e, e"),
        "parameter declared twice:\n{output}"
    );
}

#[test]
fn inline_arguments_default_skips_empty_decl_name_taken_by_another_param() {
    let input = r#"
function foo(e) {
  var n, e;
  return use(n, arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 1);
}
"#;
    let expected = r#"
function foo(e, _param_1 = 1) {
  var n, e;
  return use(n, _param_1);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_default_var_redeclaring_another_param_through_pipeline() {
    let input = r#"
var b = function (e) {
  var n = e.items, t = e.mode, e = 1 < arguments.length && void 0 !== arguments[1] ? arguments[1] : fallback;
  1 === t ? skip(n) : e(n);
};
use(b);
"#;
    let expected = r#"
const b = function(e, _param_1 = fallback) {
    const { items, mode } = e;
    var e = _param_1;
    if (mode === 1) {
        skip(items);
    } else {
        e(items);
    }
};
use(b);
"#;
    assert_eq_normalized(&common::render(input), expected);
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
fn inline_arguments_defaults_use_unused_empty_decl_names() {
    let input = r#"
function greet() {
  let name, count;
  return use(
    arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "world",
    arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 1
  );
}
"#;
    let expected = r#"
function greet(name = "world", count = 1) {
  return use(name, count);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn inline_arguments_optional_alias_uses_unused_empty_decl_name() {
    let input = r#"
function range() {
  let start, end;
  return use(
    arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : 0,
    arguments.length > 1 ? arguments[1] : undefined
  );
}
"#;
    let expected = r#"
function range(start = 0, end) {
  return use(start, end);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn inline_arguments_default_keeps_referenced_empty_decl_name() {
    let input = r#"
function foo() {
  let name;
  return use(
    name,
    arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "world"
  );
}
"#;
    let expected = r#"
function foo(_param_0 = "world") {
  let name;
  return use(name, _param_0);
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
fn destructuring_alias_does_not_capture_existing_param_default_reference() {
    let input = r#"
const enabled = true;
function select(value, options = { enabled }) {
  let { enabled } = options;
  return enabled ? value : null;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn object_property_aliases_become_param_pattern() {
    let input = r#"
function pick(_ref = {}) {
  let name = _ref.name;
  let _ref$age = _ref.age;
  let age = _ref$age === undefined ? 0 : _ref$age;
  return use(name, age);
}
"#;
    let expected = r#"
function pick({ name, age = 0 } = {}) {
  return use(name, age);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_default_referencing_later_param_stays_in_body() {
    let input = r#"
function read(options, state) {
  var current = options.value;
  var result = current === undefined ? state.fallback : current;
  return result;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn object_property_default_referencing_its_binding_stays_in_body() {
    let input = r#"
function choose(options) {
  var temporary = options.value;
  var value = temporary === undefined ? value ?? null : temporary;
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn object_property_default_referencing_earlier_binding_still_folds() {
    let input = r#"
function choose(options) {
  var first = options.first;
  var temporary = options.second;
  var second = temporary === undefined ? first : temporary;
  return [first, second];
}
"#;
    let expected = r#"
function choose({ first, second = first }) {
  return [first, second];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_default_tracks_earlier_binding_rename() {
    let input = r#"
function choose(options) {
  var n = options.type;
  var temporary = options.value;
  var value = temporary === undefined ? n : temporary;
  return [n, value];
}
"#;
    let expected = r#"
function choose({ type, value = type }) {
  return [type, value];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_binding_rename_avoids_default_capture() {
    let input = r#"
function choose(options) {
  var n = options.type;
  var temporary = options.value;
  var value = temporary === undefined ? type : temporary;
  return [n, value];
}
"#;
    let expected = r#"
function choose({ type: n, value = type }) {
  return [n, value];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_binding_rename_avoids_nested_default_capture() {
    let input = r#"
function choose(options) {
  var n = options.type;
  var temporary = options.value;
  var value = temporary === undefined ? () => type : temporary;
  return [n, value];
}
"#;
    let expected = r#"
function choose({ type: n, value = () => type }) {
  return [n, value];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_default_referencing_later_binding_stays_in_body() {
    let input = r#"
function choose(options) {
  var temporary = options.first;
  var first = temporary === undefined ? second : temporary;
  var second = options.second;
  return [first, second];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn computed_object_property_alias_becomes_param_pattern() {
    let input = r#"
function pick(property_key, _ref = {}) {
  let _ref$property_key = _ref[property_key];
  let value = _ref$property_key === undefined ? fallback : _ref$property_key;
  return use(value);
}
"#;
    let expected = r#"
function pick(property_key, { [property_key]: value = fallback } = {}) {
  return use(value);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn computed_object_property_through_generated_default_alias_becomes_param_pattern() {
    let input = r#"
function pick(property_key, _temp) {
  let _ref = _temp === undefined ? {} : _temp;
  let _ref$property_key = _ref[property_key];
  let value = _ref$property_key === undefined ? fallback : _ref$property_key;
  return use(value);
}
"#;
    let expected = r#"
function pick(property_key, { [property_key]: value = fallback } = {}) {
  return use(value);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn computed_object_property_alias_before_later_param_stays_in_body() {
    let input = r#"
function pick(property_key, _ref = {}, later = (property_key = "other")) {
  let _ref$property_key = _ref[property_key];
  let value = _ref$property_key === undefined ? fallback : _ref$property_key;
  return use(value, later);
}
"#;
    let expected = r#"
function pick(property_key, _ref = {}, later = property_key = "other") {
  let _ref$property_key = _ref[property_key];
  let value = _ref$property_key === undefined ? fallback : _ref$property_key;
  return use(value, later);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn computed_object_property_alias_with_non_param_key_stays_in_body() {
    let input = r#"
function pick(_ref = {}) {
  let _ref$property_key = _ref[property_key];
  let value = _ref$property_key === undefined ? fallback : _ref$property_key;
  return use(value);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn object_property_inline_default_aliases_become_param_pattern() {
    let input = r#"
function pick(_ref = {}) {
  let name = _ref.name;
  let _ref$age = _ref.age;
  let age;
  return use(name, _ref$age === undefined ? 0 : _ref$age);
}
"#;
    let expected = r#"
function pick({ name, age = 0 } = {}) {
  return use(name, age);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_object_property_inline_default_aliases_become_param_pattern() {
    let input = r#"
function pick() {
  let _ref = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : {};
  let name = _ref.name;
  let _ref$age = _ref.age;
  let age;
  return use(name, _ref$age === undefined ? 0 : _ref$age);
}
"#;
    let expected = r#"
function pick({ name, age = 0 } = {}) {
  return use(name, age);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arguments_object_property_default_alias_becomes_param_pattern() {
    let input = r#"
function config() {
  let _ref$mode = (arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : {}).mode;
  let appMode;
  return use(_ref$mode === undefined ? "prod" : _ref$mode);
}
"#;
    let expected = r#"
function config({ mode: appMode = "prod" } = {}) {
  return use(appMode);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn generated_param_object_property_default_alias_becomes_param_pattern() {
    let input = r#"
function config(_param_0 = {}) {
  let _ref$mode = _param_0.mode;
  let appMode;
  return use(_ref$mode === undefined ? "prod" : _ref$mode);
}
"#;
    let expected = r#"
function config({ mode: appMode = "prod" } = {}) {
  return use(appMode);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn conditional_object_property_default_alias_becomes_param_pattern() {
    let input = r#"
function config(_temp) {
  let _ref;
  let _ref$mode = (_temp === undefined ? {} : _temp).mode;
  let appMode;
  return use(_ref$mode === undefined ? "prod" : _ref$mode);
}
"#;
    let expected = r#"
function config({ mode: appMode = "prod" } = {}) {
  return use(appMode);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn const_conditional_object_property_default_alias_becomes_param_pattern() {
    let input = r#"
function config(_a) {
  let _b;
  const mode = (_a === undefined ? {} : _a).mode;
  const appMode = mode === undefined ? "prod" : mode;
  return use(appMode);
}
"#;
    let expected = r#"
function config({ mode: appMode = "prod" } = {}) {
  return use(appMode);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_alias_with_rename_becomes_param_pattern() {
    let input = r#"
function config(_ref = {}) {
  let appMode = _ref.mode;
  return use(appMode);
}
"#;
    let expected = r#"
function config({ mode: appMode } = {}) {
  return use(appMode);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_short_alias_renames_to_property_name() {
    let input = r#"
function reducer(e = s, t = {}) {
  var n = t.type;
  var r = t.payload;
  if (n === LOCATION_CHANGE) {
    return {
      ...e,
      location: r
    };
  }
  return e;
}
"#;
    let expected = r#"
function reducer(e = s, { type, payload: r } = {}) {
  if (type === LOCATION_CHANGE) {
    return {
      ...e,
      location: r
    };
  }
  return e;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_numbered_generated_alias_renames_to_property_name() {
    let input = r#"
function reducer(e = s, t = {}) {
  var ab1 = t.type;
  if (ab1 === LOCATION_CHANGE) {
    return e;
  }
  return e;
}
"#;
    let expected = r#"
function reducer(e = s, { type } = {}) {
  if (type === LOCATION_CHANGE) {
    return e;
  }
  return e;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_short_alias_rename_avoids_nested_capture() {
    let input = r#"
function reducer(t = {}) {
  var n = t.type;
  return function(type) {
    return n + type;
  };
}
"#;
    let expected = r#"
function reducer({ type: n } = {}) {
  return function(type) {
    return n + type;
  };
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_aliases_keep_body_when_param_used_later() {
    let input = r#"
function config(e) {
  const key = e.key;
  if (key === "css") {
    return e.container;
  }
  return use(e.nonce, key);
}
"#;
    let expected = r#"
function config(e) {
  const key = e.key;
  if (key === "css") {
    return e.container;
  }
  return use(e.nonce, key);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_aliases_keep_uninit_locals_used_in_closure() {
    let input = r#"
function U(e, t, n, r, o) {
  var i;
  var a;
  var u;
  var l;
  var c;
  var s = o.areStatesEqual;
  var f = o.areOwnPropsEqual;
  var d = o.areStatePropsEqual;
  var p = false;
  function h(o, p) {
    const h = !f(p, a);
    const m = !s(o, i);
    i = o;
    a = p;
    u = e(i, a);
    l = t(r, a);
    return c = n(u, l, a);
  }
  return function(o, s) {
    return p ? h(o, s) : c;
  };
}
"#;
    let expected = input;
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
fn nested_object_reassignment_default_folds_into_param() {
    // Babel 7.8 spec output: reassigns the destructured binding instead of
    // using a fresh temp, so the default appears as a body ExprStmt rather
    // than a VarDecl initializer.
    let input = r#"
function nested({ outer: _ref$outer } = {}) {
  _ref$outer = _ref$outer === undefined ? {} : _ref$outer;
  let _ref$outer$value = _ref$outer.value;
  let value = _ref$outer$value === undefined ? fallbackValue : _ref$outer$value;
  return use(value);
}
"#;
    let expected = r#"
function nested({ outer: { value = fallbackValue } = {} } = {}) {
  return use(value);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_object_reassignment_default_no_inner_props() {
    // Same reassignment pattern but without further property accesses —
    // just promote the default.
    let input = r#"
function nested({ outer: _ref$outer } = {}) {
  _ref$outer = _ref$outer === undefined ? {} : _ref$outer;
  return use(_ref$outer);
}
"#;
    let expected = r#"
function nested({ outer: _ref$outer = {} } = {}) {
  return use(_ref$outer);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_object_reassignment_default_skipped_when_binding_used_after() {
    // The binding is used after the reassignment AND in property accesses,
    // so removing the reassignment would change semantics.
    let input = r#"
function nested({ outer: _ref$outer } = {}) {
  _ref$outer = _ref$outer === undefined ? {} : _ref$outer;
  let val = _ref$outer.value;
  log(_ref$outer);
  return use(val);
}
"#;
    let expected = r#"
function nested({ outer: _ref$outer = {} } = {}) {
  let val = _ref$outer.value;
  log(_ref$outer);
  return use(val);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn pattern_c_dead_alias_inlined_enables_nested_folding() {
    // Babel 7.8 loose: Pattern C promotes the default but leaves a dead
    // alias `let _ref = _temp`. Inlining it lets property folding proceed.
    let input = r#"
function nested(_temp) {
  let _ref = _temp === void 0 ? {} : _temp;
  let _ref$outer = _ref.outer;
  _ref$outer = _ref$outer === void 0 ? {} : _ref$outer;
  let _ref$outer$value = _ref$outer.value;
  let value = _ref$outer$value === void 0 ? fallbackValue : _ref$outer$value;
  return use(value);
}
"#;
    let expected = r#"
function nested({ outer: { value = fallbackValue } = {} } = {}) {
  return use(value);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn pattern_c_dead_alias_not_inlined_when_param_reassigned() {
    // _temp is reassigned after the alias — inlining would lose the capture.
    let input = r#"
function f(_temp) {
  let _ref = _temp === void 0 ? {} : _temp;
  _temp = other;
  return use(_ref);
}
"#;
    let expected = r#"
function f(_temp = {}) {
  let _ref = _temp;
  _temp = other;
  return use(_ref);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn pattern_c_dead_alias_not_inlined_when_alias_reassigned() {
    // _ref is reassigned — it's not a dead alias.
    let input = r#"
function f(_temp) {
  let _ref = _temp === void 0 ? {} : _temp;
  _ref = modified;
  return use(_ref);
}
"#;
    let expected = r#"
function f(_temp = {}) {
  let _ref = _temp;
  _ref = modified;
  return use(_ref);
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
fn array_index_aliases_become_param_pattern() {
    let input = r#"
function first(_ref = []) {
  let head = _ref[0];
  let _ref$ = _ref[1];
  let second = _ref$ === undefined ? fallback : _ref$;
  return use(head, second);
}
"#;
    let expected = r#"
function first([head, second = fallback] = []) {
  return use(head, second);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn array_index_aliases_keep_body_when_param_used_later() {
    let input = r#"
function first(items) {
  const head = items[0];
  return use(head, items.length);
}
"#;
    let expected = r#"
function first(items) {
  const head = items[0];
  return use(head, items.length);
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn array_index_default_referencing_later_param_stays_in_body() {
    let input = r#"
function first(items, fallback) {
  var temporary = items[0];
  var value = temporary === undefined ? fallback : temporary;
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn conditional_array_index_aliases_become_param_pattern() {
    let input = r#"
function first(_ref) {
  let head = (_ref === undefined ? [] : _ref)[0];
  let _ref$ = _ref[1];
  let second = _ref$ === undefined ? fallback : _ref$;
  return use(head, second);
}
"#;
    let expected = r#"
function first([head, second = fallback] = []) {
  return use(head, second);
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
fn computed_destructuring_before_later_param_stays_in_body() {
    let input = r#"
function select(source, key) {
  const { [key]: picked, ...rest } = source;
  return [picked, rest];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_computed_destructuring_before_later_param_stays_in_body() {
    let input = r#"
const select = (source, key) => {
  const { [key]: picked, ...rest } = source;
  return [picked, rest];
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructuring_default_referencing_later_param_stays_in_body() {
    let input = r#"
function select(source, fallback) {
  const { value = fallback } = source;
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructuring_default_referencing_its_binding_stays_in_body() {
    let input = r#"
function select(source) {
  var { value = value ?? null } = source;
  return value;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_destructuring_default_referencing_later_param_stays_in_body() {
    let input = r#"
const select = (source, fallback) => {
  const { value = fallback } = source;
  return value;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_destructuring_default_referencing_its_binding_stays_in_body() {
    let input = r#"
const select = (source) => {
  var { value = value ?? null } = source;
  return value;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructuring_expressions_from_earlier_params_fold_when_alias_is_last() {
    let input = r#"
function select(key, fallback, source) {
  const { [key]: picked, value = fallback } = source;
  return [picked, value];
}
"#;
    let expected = r#"
function select(key, fallback, { [key]: picked, value = fallback }) {
  return [picked, value];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arrow_computed_destructuring_with_outer_key_still_folds() {
    let input = r#"
const select = (source) => {
  const { [propertyKey]: picked, ...rest } = source;
  return [picked, rest];
};
"#;
    let expected = r#"
const select = ({ [propertyKey]: picked, ...rest }) => {
  return [picked, rest];
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn destructured_param_alias_after_bare_decls_becomes_param_pattern() {
    let input = r#"
function foo(_ref) {
  let cache;
  let state;
  const { name, value = fallback } = _ref;
  cache = {};
  state = value;
  return name;
}
"#;
    let expected = r#"
function foo({ name, value = fallback }) {
  let cache;
  let state;
  cache = {};
  state = value;
  return name;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn destructured_param_alias_after_bare_decl_referencing_decl_stays_in_body() {
    let input = r#"
function foo(_ref) {
  let fallback;
  const { value = fallback } = _ref;
  return value;
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

#[test]
fn inline_arguments_default_preserves_candidate_binding_context() {
    let input = r#"
function greet() {
  let name;
  return arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "world";
}
"#;

    let (candidate_ctxt, param_ctxt) = recovered_first_param_context(input);

    assert_eq!(
        param_ctxt, candidate_ctxt,
        "recovered parameter should keep the resolver binding context from the consumed declaration"
    );
    assert_ne!(
        param_ctxt,
        SyntaxContext::empty(),
        "regression input should use a scoped local binding"
    );
}

#[test]
fn placeholder_param_declaration_and_references_share_one_context() {
    // No body declaration names the second argument, so the rule invents
    // `_param_1`. Both reads must resolve to the one parameter it declares:
    // a reference with a different context would be invisible to every later
    // `(sym, ctxt)`-keyed pass, and a rename of the parameter would leave it
    // behind as a free name.
    let input = r#"
function foo(a) {
  use(arguments.length > 1 ? arguments[1] : void 0);
  use(arguments.length > 1 ? arguments[1] : void 0);
  return a;
}
"#;
    let expected = r#"
function foo(a, _param_1) {
  use(_param_1);
  use(_param_1);
  return a;
}
"#;
    assert_eq_normalized(&apply(input), expected);

    let ctxts = ident_contexts_after_rule(input, "_param_1");
    assert_eq!(
        ctxts.len(),
        3,
        "one declaration and two references: {ctxts:?}"
    );
    assert!(
        ctxts.iter().all(|ctxt| *ctxt == ctxts[0]),
        "declaration and references must share one context: {ctxts:?}"
    );
    assert_ne!(ctxts[0], SyntaxContext::empty());
}

fn ident_contexts_after_rule(input: &str, name: &str) -> Vec<SyntaxContext> {
    struct Contexts<'a> {
        name: &'a str,
        found: Vec<SyntaxContext>,
    }
    impl Visit for Contexts<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if ident.sym == self.name {
                self.found.push(ident.ctxt);
            }
        }
    }

    GLOBALS.set(&Default::default(), || {
        let mut module = parse_and_resolve(input);
        let unresolved_mark = module.1;
        module.0.visit_mut_with(&mut UnParameters::new(
            unresolved_mark,
            RewriteLevel::Standard,
        ));
        let mut contexts = Contexts {
            name,
            found: Vec::new(),
        };
        module.0.visit_with(&mut contexts);
        contexts.found
    })
}

fn parse_and_resolve(input: &str) -> (swc_core::ecma::ast::Module, Mark) {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        FileName::Custom("fixture.js".to_string()).into(),
        input.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        EsVersion::latest(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let mut module = parser.parse_module().expect("input should parse");
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    (module, unresolved_mark)
}

fn recovered_first_param_context(input: &str) -> (SyntaxContext, SyntaxContext) {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(
            FileName::Custom("fixture.js".to_string()).into(),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                jsx: true,
                ..Default::default()
            }),
            EsVersion::latest(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        let mut module = parser.parse_module().expect("input should parse");

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let candidate_ctxt = first_body_var_context(first_function(&module).function.as_ref());

        module.visit_mut_with(&mut UnParameters::new(
            unresolved_mark,
            RewriteLevel::Standard,
        ));

        (
            candidate_ctxt,
            first_param_context(first_function(&module).function.as_ref()),
        )
    })
}

fn first_function(module: &swc_core::ecma::ast::Module) -> &swc_core::ecma::ast::FnDecl {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = &module.body[0] else {
        panic!("expected function declaration");
    };
    function
}

fn first_body_var_context(function: &Function) -> SyntaxContext {
    let body = function.body.as_ref().expect("expected function body");
    let Stmt::Decl(Decl::Var(var)) = &body.stmts[0] else {
        panic!("expected first body statement to be a var declaration");
    };
    let Pat::Ident(BindingIdent { id, .. }) = &var.decls[0].name else {
        panic!("expected identifier declaration");
    };
    id.ctxt
}

fn first_param_context(function: &Function) -> SyntaxContext {
    let Pat::Assign(assign) = &function.params[0].pat else {
        panic!("expected recovered default parameter");
    };
    let Pat::Ident(BindingIdent { id, .. }) = assign.left.as_ref() else {
        panic!("expected identifier parameter");
    };
    id.ctxt
}

// --- defaults change `arguments` mapping and prologue order ---

#[test]
fn guard_stays_when_function_reads_mapped_arguments() {
    // With a simple parameter list, `a = 2` also writes `arguments[0]`.
    // A default parameter unmaps `arguments`, so `f(1)` would return 1.
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  a = 2;
  return arguments[0];
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn guard_stays_when_nested_arrow_reads_mapped_arguments() {
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  var g = () => arguments[0];
  return g();
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_arguments_escapes() {
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  return Array.prototype.slice.call(arguments);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_direct_eval_may_read_arguments() {
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  return eval(source);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn tail_arguments_reads_do_not_block_default() {
    // `arguments.length` and slots past the parameter list are never mapped.
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  return [a, arguments.length, arguments[1]];
}
"#;
    let expected = r#"
function f(a = 3) {
  return [a, arguments.length, arguments[1]];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_function_arguments_do_not_block_default() {
    let input = r#"
function f(a) {
  if (a === void 0) { a = 3; }
  return function() { return arguments[0]; };
}
"#;
    let expected = r#"
function f(a = 3) {
  return function() { return arguments[0]; };
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guard_after_observable_statement_stays() {
    // `console.log(a)` runs before the guard and sees `undefined`; a default
    // would run first and it would see 3.
    let input = r#"
function f(a) {
  console.log(a);
  if (a === void 0) { a = 3; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn guards_out_of_parameter_order_convert_only_the_prefix() {
    // Defaults evaluate in parameter order, so `a`'s impure default cannot
    // move ahead of `b`'s once `b`'s guard has been converted.
    let input = r#"
function f(a, b) {
  if (b === void 0) { b = g(); }
  if (a === void 0) { a = h(); }
  return a + b;
}
"#;
    let expected = r#"
function f(a, b = g()) {
  if (a === void 0) { a = h(); }
  return a + b;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arrow_guard_after_observable_statement_stays() {
    let input = r#"
const f = (a) => {
  console.log(a);
  if (a === void 0) { a = 3; }
  return a;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

// --- hoisting a pure default past statements that cannot observe it ---

#[test]
fn pure_default_hoists_past_unrelated_prologue_statements() {
    // TypeScript emits `var _this = this;` and hoisted declarations ahead of
    // the guards; none of them mention `n`, and `[]` has no effects.
    let input = r#"
function f(e, n) {
  var self = this;
  let t;
  if (!e) return;
  if (n === void 0) { n = []; }
  return [self, t, e, n];
}
"#;
    let expected = r#"
function f(e, n = []) {
  var self = this;
  let t;
  if (!e) return;
  return [self, t, e, n];
}
"#;
    assert_eq_normalized(&apply(input), expected);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), expected);
}

#[test]
fn guard_stays_when_param_is_written_before_it() {
    // ws-style: `K = undefined` runs before the guard, so the guard re-defaults
    // a value the body cleared. A default parameter would not.
    let input = r#"
function f(q, K, _) {
  if (typeof q === "function") {
    _ = q;
    q = K = undefined;
  }
  if (K === void 0) { K = !this._isServer; }
  return [q, K, _];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_param_is_read_before_it() {
    let input = r#"
function f(a) {
  const seen = a;
  if (a === void 0) { a = 3; }
  return [seen, a];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_closure_created_before_it_mentions_param() {
    let input = r#"
function f(a) {
  const read = () => a;
  if (a === void 0) { a = 3; }
  return read;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_hoisted_function_declaration_mentions_param() {
    // `g` is hoisted, so `g()` before the guard may write `a`.
    let input = r#"
function f(a) {
  g();
  if (a === void 0) { a = 3; }
  return a;
  function g() { a = 5; }
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn impure_default_does_not_hoist_past_other_statements() {
    let input = r#"
function f(a) {
  log("start");
  if (a === void 0) { a = compute(); }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn member_read_default_hoists_only_at_standard() {
    // `this._isServer` is a property read: `pure_getters` at standard.
    let input = r#"
function f(a) {
  let t;
  if (a === void 0) { a = !this._isServer; }
  return [t, a];
}
"#;
    let expected = r#"
function f(a = !this._isServer) {
  let t;
  return [t, a];
}
"#;
    assert_eq_normalized(&apply(input), expected);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn object_literal_default_hoists_past_other_param_normalization() {
    let input = r#"
function f(a, b) {
  if (typeof a === "string") { a = a.toUpperCase(); } else if (a === void 0) { a = "X"; }
  if (b === void 0) { b = { parse: true }; }
  return [a, b];
}
"#;
    let expected = r#"
function f(a, b = { parse: true }) {
  if (typeof a === "string") { a = a.toUpperCase(); } else if (a === void 0) { a = "X"; }
  return [a, b];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn arrow_pure_default_hoists_past_unrelated_statement() {
    let input = r#"
const f = (a, b) => {
  if (!a) return;
  if (b === void 0) { b = false; }
  return [a, b];
};
"#;
    let expected = r#"
const f = (a, b = false) => {
  if (!a) return;
  return [a, b];
};
"#;
    assert_eq_normalized(&apply(input), expected);
}

// --- the hoisted default must read the same values ---

#[test]
fn guard_stays_when_a_param_the_default_reads_is_written_before_it() {
    // `seed = 1` runs before the guard, so the guard assigns 1. A default
    // `a = seed` would read the call-time 0.
    let input = r#"
function f(seed, a) {
  seed = 1;
  if (a === void 0) { a = seed; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn default_reading_an_untouched_param_hoists_past_unrelated_statement() {
    let input = r#"
function f(a, b) {
  let t;
  if (!a) return;
  if (b === void 0) { b = a; }
  return [t, b];
}
"#;
    let expected = r#"
function f(a, b = a) {
  let t;
  if (!a) return;
  return [t, b];
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guard_stays_when_default_reads_outer_binding_past_a_call() {
    // `tick()` may write the module-level `counter` the default reads.
    let input = r#"
let counter = 0;
function tick() { counter++; }
function f(a) {
  tick();
  if (a === void 0) { a = counter; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn default_reading_outer_binding_hoists_past_inert_declarations() {
    let input = r#"
const defaults = { x: 1 };
function f(a) {
  var self = this;
  let t;
  if (a === void 0) { a = defaults; }
  return [self, t, a];
}
"#;
    let expected = r#"
const defaults = { x: 1 };
function f(a = defaults) {
  var self = this;
  let t;
  return [self, t, a];
}
"#;
    assert_eq_normalized(&apply(input), expected);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), expected);
}

#[test]
fn guard_stays_when_property_read_default_follows_a_property_write() {
    let input = r#"
function f(a) {
  this.flag = true;
  if (a === void 0) { a = !this.flag; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_global_read_default_follows_a_call() {
    let input = r#"
function f(a) {
  configure();
  if (a === void 0) { a = globalConfig; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn closure_default_hoists_past_a_call() {
    // The arrow reads `counter` when it is called, not when the default runs.
    let input = r#"
let counter = 0;
function f(a) {
  tick();
  if (a === void 0) { a = () => counter; }
  return a;
}
"#;
    let expected = r#"
let counter = 0;
function f(a = () => counter) {
  tick();
  return a;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn guard_stays_when_closure_before_it_may_write_the_default_dependency() {
    let input = r#"
function f(seed, a) {
  const bump = () => { seed = 1; };
  if (a === void 0) { a = seed; }
  return [bump, a];
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn guard_stays_when_default_dependency_is_updated_before_it() {
    let input = r#"
function f(count, a) {
  count++;
  if (a === void 0) { a = count; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn default_reading_outer_binding_hoists_past_object_destructuring_at_standard() {
    // `let { top, left } = rect;` only reads properties (`pure_getters`).
    let input = r#"
const Vs = { x: 0.2 };
function f(rect, a) {
  let { top, left } = rect;
  if (a === void 0) { a = Vs; }
  return [top, left, a];
}
"#;
    // At standard the later destructured-parameter fold also consumes the
    // declaration; at minimal the destructuring statement is not inert.
    let expected = r#"
const Vs = { x: 0.2 };
function f({ top, left }, a = Vs) {
  return [top, left, a];
}
"#;
    assert_eq_normalized(&apply(input), expected);
    assert_eq_normalized(&apply_with_level(input, RewriteLevel::Minimal), input);
}

#[test]
fn throwing_default_does_not_hoist_past_an_early_return() {
    // `+1n` throws a TypeError. Hoisted, it would run on the `skip` path that
    // returned before reaching the guard.
    let input = r#"
function f(skip, a) {
  if (skip) return 1;
  if (a === void 0) { a = +1n; }
  return a;
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn negated_bigint_default_still_hoists() {
    let input = r#"
function f(skip, a) {
  if (skip) return 1;
  if (a === void 0) { a = -1n; }
  return a;
}
"#;
    let expected = r#"
function f(skip, a = -1n) {
  if (skip) return 1;
  return a;
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_property_short_alias_rename_avoids_computed_key_capture() {
    // `type` is read in a computed key after the destructuring; renaming `n`
    // to `type` would make that key read the parameter instead.
    let input = r#"
function reducer(t = {}) {
  var n = t.type;
  return { [type]: n };
}
"#;
    let expected = r#"
function reducer({ type: n } = {}) {
  return {
    [type]: n
  };
}
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn property_alias_read_by_a_later_param_default_stays_in_body() {
    // `n = e.currentTarget` reads the parameter the fold would replace with a
    // destructuring pattern; a default cannot see the body, so `e` would dangle.
    let input = r#"
function capture(e, t, n = e.currentTarget) {
  let r = e.nativeEvent;
  return use(r, t, n);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn destructuring_alias_read_by_a_later_param_default_stays_in_body() {
    let input = r#"
function capture(e, t, n = e.currentTarget) {
  let { nativeEvent } = e;
  return use(nativeEvent, t, n);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn array_index_alias_read_by_a_later_param_default_stays_in_body() {
    let input = r#"
function capture(e, n = e.length) {
  let first = e[0];
  let second = e[1];
  return use(first, second, n);
}
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn arrow_destructuring_alias_read_by_a_later_param_default_stays_in_body() {
    let input = r#"
const capture = (e, t, n = e.currentTarget) => {
  let { nativeEvent } = e;
  return use(nativeEvent, t, n);
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn reassignment_default_on_a_binding_read_by_a_later_param_default_stays_in_body() {
    // Babel 7.8 reassignment default on a destructured binding. The sibling
    // default reads `_ref$outer` before the body reassignment runs, so the
    // default cannot move into the pattern, and folding the property read into
    // a nested pattern would remove the binding that default reads.
    let input = r#"
function nested({ outer: _ref$outer } = {}, mode = _ref$outer.mode) {
  _ref$outer = _ref$outer === undefined ? {} : _ref$outer;
  let _ref$outer$value = _ref$outer.value;
  let value = _ref$outer$value === undefined ? fallbackValue : _ref$outer$value;
  return use(value, mode);
}
"#;
    assert_eq_normalized(&apply(input), input);
}
