mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::{rules::UnNullishCoalescing, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |unresolved_mark| {
        UnNullishCoalescing::new(unresolved_mark, level)
    })
}

#[test]
fn preserves_existing_nullish_assignment() {
    let input = r#"foo ??= "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_identifier_nullish_assignment_form() {
    let input = r#"foo ?? (foo = "bar")"#;
    let expected = r#"foo ??= "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_unresolved_identifier_nullish_assignment_form_at_minimal() {
    let input = r#"foo ?? (foo = "bar")"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_local_identifier_nullish_assignment_form_at_minimal() {
    let input = r#"let foo;
foo ?? (foo = "bar")"#;
    let expected = r#"let foo;
foo ??= "bar""#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_unresolved_identifier_nullish_assignment_form_at_standard() {
    let input = r#"foo ?? (foo = "bar")"#;
    let expected = r#"foo ??= "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_mismatched_nullish_assignment_target() {
    let input = r#"foo ?? (bar = "bar")"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_member_nullish_assignment_form_at_standard() {
    // Unresolved member bases can behave like global/dynamic references.
    let input = r#"obj.value ?? (obj.value = "bar")"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_static_local_base_member_nullish_assignment_form_at_standard() {
    let input = r#"let obj;
obj.value ?? (obj.value = "bar")"#;
    let expected = r#"let obj;
obj.value ??= "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_static_local_base_member_nullish_assignment_form_at_minimal() {
    let input = r#"let obj;
obj.value ?? (obj.value = "bar")"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_member_nullish_assignment_form_at_aggressive() {
    let input = r#"obj.value ?? (obj.value = "bar")"#;
    let expected = r#"obj.value ??= "bar""#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_nested_member_nullish_assignment_form_at_standard() {
    let input = r#"let obj;
obj.meta.value ?? (obj.meta.value = "bar")"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_nested_member_nullish_assignment_form_at_aggressive() {
    let input = r#"let obj;
obj.meta.value ?? (obj.meta.value = "bar")"#;
    let expected = r#"let obj;
obj.meta.value ??= "bar""#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_computed_member_nullish_assignment_form_at_standard() {
    let input = r#"let obj;
obj[key] ?? (obj[key] = "bar")"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_computed_member_nullish_assignment_form_at_aggressive() {
    let input = r#"let obj;
obj[key] ?? (obj[key] = "bar")"#;
    let expected = r#"let obj;
obj[key] ??= "bar""#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_call_base_member_nullish_assignment_form_at_aggressive() {
    let input = r#"getObj().value ?? (getObj().value = "bar")"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, input);
}

#[test]
fn pipeline_recovers_strict_nullish_assignment_lowering() {
    // Reproduced by TypeScript 5.9.3 and @swc/core 1.15.41 targeting ES5 for:
    // `let cache; const out = cache ??= make();`
    let input = r#"
var cache;
var out = cache !== null && cache !== void 0 ? cache : cache = make();
"#;
    let expected = r#"
let cache;
const out = cache ??= make();
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_temp_backed_computed_member_nullish_assignment_at_minimal() {
    let input = r#"
var _obj, _key;
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[_key] = make());
"#;
    let expected = r#"
var out = getObj()[getKey()] ??= make();
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn pipeline_recovers_computed_member_nullish_assignment_lowering() {
    // Reproduced by Babel, TypeScript, SWC, and esbuild when lowering:
    // `const out = getObj()[getKey()] ??= make();`
    let input = r#"
var _value, _obj, _key;
var out = (_value = (_obj = getObj())[_key = getKey()]) !== null &&
    _value !== void 0
    ? _value
    : _obj[_key] = make();
use(out);
"#;
    let expected = r#"
const out = getObj()[getKey()] ??= make();
use(out);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_temp_backed_computed_member_when_object_temp_escapes() {
    let input = r#"
var _obj, _key;
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[_key] = make());
use(_obj);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_temp_backed_computed_member_when_key_temp_escapes() {
    let input = r#"
var _obj, _key;
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[_key] = make());
use(_key);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_temp_backed_computed_member_with_mismatched_key() {
    let input = r#"
var _obj, _key;
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[other] = make());
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_temp_backed_computed_member_without_temp_declarations() {
    let input = r#"
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[_key] = make());
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_temp_backed_computed_member_with_initialized_temp() {
    let input = r#"
var _obj = previous, _key;
var out = (_obj = getObj())[_key = getKey()] ?? (_obj[_key] = make());
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_not_null_and_not_undefined_ternary() {
    let input = r#"foo !== null && foo !== void 0 ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_strict_not_null_ternary_at_minimal() {
    let input = r#"foo !== null && foo !== void 0 ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_null_or_undefined_ternary_flipped() {
    let input = r#"foo === null || foo === void 0 ? "bar" : foo"#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_temp_variable_assignment_in_condition() {
    let input = r#"var _ref;
(_ref = foo) !== null && _ref !== void 0 ? _ref : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_eval_observable_consumed_temp_decl() {
    let input = r#"
function f() {
  var _ref;
  eval("_ref");
  return (_ref = foo) !== null && _ref !== void 0 ? _ref : "bar";
}
"#;
    let expected = r#"
function f() {
  var _ref;
  eval("_ref");
  return foo ?? "bar";
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_undefined_identifier_instead_of_void_0() {
    let input = r#"foo !== null && foo !== undefined ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_not_null_ternary() {
    let input = r#"foo != null ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_not_null_ternary_at_minimal() {
    let input = r#"foo != null ? foo : "bar""#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_loose_null_ternary_flipped() {
    let input = r#"foo == null ? "bar" : foo"#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_null_ternary_flipped_at_minimal() {
    let input = r#"foo == null ? "bar" : foo"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_loose_temp_variable_assignment_in_condition() {
    let input = r#"var _ref;
(_ref = foo) != null ? _ref : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_temp_variable_assignment_at_minimal() {
    let input = r#"var _ref;
(_ref = foo) != null ? _ref : "bar""#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_non_null_check_ternary() {
    let input = r#"foo ? bar : baz"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_mismatched_variable() {
    let input = r#"foo !== null && bar !== void 0 ? foo : "baz""#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_or_chain_with_temp_var_to_nullish_true() {
    // (tmp = expr) === null || tmp === undefined || tmp  →  expr ?? true
    let input = r#"var G;
(G = B.broadcast) === null || G === undefined || G"#;
    let expected = r#"B.broadcast ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_or_chain_with_temp_var_at_minimal() {
    // Temp usage proves single evaluation, so this does not need the Standard
    // repeated-read heuristic.
    let input = r#"var G;
(G = B.broadcast) === null || G === undefined || G"#;
    let expected = r#"B.broadcast ?? true"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_or_chain_plain_form_to_nullish_true() {
    // x === null || x === undefined || x  →  x ?? true
    let input = r#"foo === null || foo === undefined || foo"#;
    let expected = r#"foo ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_or_chain_plain_form_at_minimal() {
    // The plain form collapses repeated identifier reads and is a Standard-level
    // heuristic. The temp form remains allowed at Minimal.
    let input = r#"foo === null || foo === undefined || foo"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_or_chain_with_void_0_to_nullish_true() {
    let input = r#"foo === null || foo === void 0 || foo"#;
    let expected = r#"foo ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_or_chain_with_mismatched_tail() {
    // tail must match the checked variable
    let input = r#"foo === null || foo === undefined || bar"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_or_chain_with_mismatched_checks() {
    let input = r#"foo === null || bar === undefined || foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_or_chain_with_optional_chaining_assignment() {
    // Real-world esbuild pattern: (tmp = obj?.prop) === null || tmp === undefined || tmp
    let input = r#"var G;
(G = B?.broadcast) === null || G === undefined || G"#;
    let expected = r#"B?.broadcast ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_negated_or_chain() {
    // Negated form: !((tmp = obj?.prop) === null || tmp === undefined || tmp)
    let input = r#"var G;
!((G = B?.redirect) === null || G === undefined || G)"#;
    let expected = r#"!(B?.redirect ?? true)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_or_chain_flipped_comparisons() {
    // Exact shape from issue-52 after FlipComparisons: null on the left
    let input = r#"var G;
null === (G = null == B ? void 0 : B.broadcast) || void 0 === G || G"#;
    let expected = r#"(null == B ? void 0 : B.broadcast) ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_or_chain_plain_member_expression_at_standard() {
    // Member expressions read the property multiple times — collapsing to
    // a single read changes semantics for getters/proxies.
    let input = r#"B.broadcast === null || B.broadcast === undefined || B.broadcast"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_or_chain_plain_member_expression_at_aggressive() {
    let input = r#"B.broadcast === null || B.broadcast === undefined || B.broadcast"#;
    let expected = r#"B.broadcast ?? true"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_temp_without_declaration() {
    let input = r#"(G = B.broadcast) === null || G === undefined || G"#;
    let output = apply(input);
    assert!(
        output.contains("G ="),
        "should not erase undeclared temp assignment: {output}"
    );
}

#[test]
fn does_not_transform_ternary_temp_without_declaration() {
    let input = r#"(_ref = foo) !== null && _ref !== void 0 ? _ref : "bar""#;
    let output = apply(input);
    assert!(
        output.contains("_ref ="),
        "should not erase undeclared temp assignment: {output}"
    );
}

#[test]
fn does_not_transform_or_chain_when_temp_used_elsewhere() {
    let input = r#"var G;
var v = (G = B.broadcast) === null || G === undefined || G;
use(G);"#;
    let output = apply(input);
    assert!(
        output.contains("G ="),
        "should preserve assignment when temp is used elsewhere: {output}"
    );
}

#[test]
fn does_not_transform_ternary_temp_when_used_elsewhere() {
    let input = r#"var _ref;
var v = (_ref = foo) !== null && _ref !== void 0 ? _ref : "bar";
use(_ref);"#;
    let output = apply(input);
    assert!(
        output.contains("_ref ="),
        "should preserve assignment when temp is used elsewhere: {output}"
    );
}

#[test]
fn does_not_transform_loose_ternary_temp_when_used_elsewhere() {
    let input = r#"var _ref;
var v = (_ref = foo) != null ? _ref : "bar";
use(_ref);"#;
    let output = apply(input);
    assert!(
        output.contains("_ref ="),
        "should preserve assignment when temp is used elsewhere: {output}"
    );
}

#[test]
fn transforms_or_chain_temp_with_declaration_only_in_pattern() {
    let input = r#"var G;
var v = (G = B.broadcast) === null || G === undefined || G;"#;
    let expected_fragment = "B.broadcast ?? true";
    let output = apply(input);
    assert!(
        output.contains(expected_fragment),
        "should transform when temp is only used in pattern: {output}"
    );
}
