mod common;

use common::{assert_eq_normalized, render_rule};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, SyntaxContext, GLOBALS};
use swc_core::ecma::ast::{BindingIdent, Decl, EsVersion, Function, ModuleItem, Pat, Stmt};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;
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
