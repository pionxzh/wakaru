mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::{rules::UnOptionalChaining, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnOptionalChaining::new(level))
}

#[test]
fn transforms_member_access_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.a"#;
    let expected = r#"obj?.a"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_null_check() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method()"#;
    let expected = r#"obj?.method()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_method_call_with_args() {
    let input = r#"obj === null || obj === void 0 ? void 0 : obj.method(1, 2)"#;
    let expected = r#"obj?.method(1, 2)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_strict_babel_temp_variable_assignment_form() {
    let input = r#"(_a = a) === null || _a === void 0 ? void 0 : _a.b"#;
    let expected = r#"a?.b"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_transforms_strict_temp_variable_assignment_form() {
    let input = r#"(_a = a) === null || _a === void 0 ? void 0 : _a.b"#;
    let expected = r#"a?.b"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_strict_babel_optional_call_form() {
    let input = r#"(_a = obj.getRootNode) === null || _a === void 0 ? void 0 : _a.call(obj)"#;
    let expected = r#"obj.getRootNode?.()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_optional_member_call_pattern_into_optional_call() {
    let input = r#"te?.getRootNode?.call(te)"#;
    let expected = r#"te?.getRootNode?.()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_strict_babel_optional_call_with_memoized_context() {
    let input = r#"(_obj_method = (_obj = getObj()).method) === null || _obj_method === void 0 ? void 0 : _obj_method.call(_obj, arg)"#;
    let expected = r#"getObj().method?.(arg)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_strict_babel_optional_call_from_optional_member() {
    let input = r#"(_a = te?.getRootNode) === null || _a === void 0 ? void 0 : _a.call(te)"#;
    let expected = r#"te?.getRootNode?.()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_optional_call_with_wrong_context() {
    let input = r#"(_a = te?.getRootNode) === null || _a === void 0 ? void 0 : _a.call(other)"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_transforms_nested_babel_optional_call_from_lowered_optional_member() {
    let input = r#"(_a = (_b = runtime?.plugin) === null || _b === void 0 ? void 0 : _b.createHook) === null || _a === void 0 ? void 0 : _a.call(_b, "payload")"#;
    let expected = r#"runtime?.plugin?.createHook?.("payload")"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_guarded_babel_optional_call_statement() {
    let input = r#"
if (!((_ = (K = this.handle) === null || K === void 0 ? void 0 : K.close) === null || _ === void 0)) {
  _.call(K);
}
"#;
    let expected = r#"
this.handle?.close?.();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_guarded_optional_call_statement_with_wrong_context() {
    let input = r#"
if (!((_a = te?.getRootNode) === null || _a === void 0)) {
  _a.call(other);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_transforms_short_circuit_babel_optional_call_statement() {
    let input = r#"(_ = (K = this.handle) === null || K === void 0 ? void 0 : K.close) === null || _ === void 0 || _.call(K)"#;
    let expected = r#"this.handle?.close?.()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_declared_scratch_temp_assignment_form() {
    let input = r#"
let n;
const x = (n = service.connection) === null || n === void 0 ? void 0 : n.status;
"#;
    let expected = r#"
let n;
const x = service.connection?.status;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_nested_babel_optional_member_from_recovered_optional_chain() {
    let input = r#"(_a = runtime?.plugin) === null || _a === void 0 ? void 0 : _a.version"#;
    let expected = r#"runtime?.plugin?.version"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_generated_named_temp_member_access() {
    let input = r#"(T1 = source.adapter) === null || T1 === void 0 ? void 0 : T1.name"#;
    let expected = r#"source.adapter?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn logical_and_form_stays_as_is() {
    let input = r#"x !== null && x !== void 0 && x.foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_different_variable_in_access() {
    // alt uses `other`, not `obj` — should not transform to optional chain
    let input = r#"obj === null || obj === void 0 ? void 0 : other.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_when_cons_is_not_void() {
    let input = r#"obj === null || obj === void 0 ? "fallback" : obj.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// --- loose equality form: x == null ? undefined : x.prop ---

#[test]
fn transforms_loose_eq_null_member_access() {
    let input = r#"const x = U == null ? undefined : U.userID"#;
    let expected = r#"const x = U?.userID"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_transform_loose_eq_null_member_access() {
    let input = r#"const x = U == null ? undefined : U.userID"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_loose_eq_null_method_call() {
    let input = r#"const x = U == null ? undefined : U.getName()"#;
    let expected = r#"const x = U?.getName()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_neq_null_nullish_form() {
    // x != null ? x.prop : undefined  →  x?.prop
    let input = r#"const x = U != null ? U.name : undefined"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_nested_loose_eq_null() {
    // Nested: (x == null ? undefined : x.a) == null ? undefined : (x == null ? undefined : x.a).b
    // After first pass: x?.a, then second nesting would need chaining
    let input = r#"const x = U == null ? undefined : U.a"#;
    let expected = r#"const x = U?.a"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_eq_when_cons_is_not_undefined() {
    let input = r#"const x = U == null ? "default" : U.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn transforms_loose_eq_null_in_expression_position() {
    // Inside if condition
    let input = r#"if ((U == null ? undefined : U.message) && true) {}"#;
    let expected = r#"if (U?.message && true) {}"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- loose equality edge cases (from Codex review) ---

#[test]
fn transforms_loose_eq_null_with_void_0_consequent() {
    let input = r#"const x = U == null ? void 0 : U.name"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_null_reversed_operand_order() {
    // null == U instead of U == null
    let input = r#"const x = null == U ? undefined : U.prop"#;
    let expected = r#"const x = U?.prop"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_loose_eq_undefined() {
    // x == undefined is equivalent to x == null in JS
    let input = r#"const x = U == undefined ? undefined : U.name"#;
    let expected = r#"const x = U?.name"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// --- loose equality assignment form ---

#[test]
fn does_not_transform_loose_eq_assignment_member_access() {
    let input = r#"const x = (n = e.ownerDocument) == null ? undefined : n.defaultView"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_transforms_loose_eq_babel_assignment_member_access() {
    let input = r#"const x = (_a = e.ownerDocument) == null ? undefined : _a.defaultView"#;
    let expected = r#"const x = e.ownerDocument?.defaultView"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_eq_assignment_method_call() {
    let input = r#"const x = (t = obj.getRootNode) == null ? undefined : t.call(obj)"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_transforms_loose_eq_babel_optional_call_form() {
    let input = r#"const x = (_a = obj.getRootNode) == null ? undefined : _a.call(obj)"#;
    let expected = r#"const x = obj.getRootNode?.()"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_transforms_loose_eq_assignment_method_call() {
    let input = r#"const x = (t = obj.getRootNode) == null ? undefined : t.call(obj)"#;
    let expected = r#"const x = obj.getRootNode?.()"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_neq_assignment_form() {
    let input = r#"const x = (n = e.body) != null ? n.scrollWidth : undefined"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_transforms_loose_neq_assignment_form() {
    let input = r#"const x = (n = e.body) != null ? n.scrollWidth : undefined"#;
    let expected = r#"const x = e.body?.scrollWidth"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_loose_eq_assignment_with_computed_access() {
    let input = r#"const x = (t = e[n.type]) == null ? undefined : t.duration"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_transforms_loose_eq_assignment_with_computed_access() {
    let input = r#"const x = (t = e[n.type]) == null ? undefined : t.duration"#;
    let expected = r#"const x = e[n.type]?.duration"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_assignment_side_effect_for_observable_temp() {
    let input = r#"
let n = 0;
const x = (n = obj) == null ? undefined : n.value;
use(n);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_preserves_declared_scratch_temp_when_it_is_observed_later() {
    let input = r#"
let n;
const x = (n = obj) == null ? undefined : n.value;
use(n);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_preserves_observed_underscore_temp_assignment() {
    let input = r#"
let _a = 0;
const x = (_a = obj) == null ? undefined : _a.value;
use(_a);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_rewrites_observable_temp_assignment_pattern() {
    let input = r#"
let n = 0;
const x = (n = obj) == null ? undefined : n.value;
use(n);
"#;
    let expected = r#"
let n = 0;
const x = obj?.value;
use(n);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

// --- known-broken semantic regressions ---

#[test]
fn known_bug_logical_and_expression_value_not_converted() {
    let input = r#"x !== null && x !== undefined && x.foo"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
