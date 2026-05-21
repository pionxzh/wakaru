mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::{rules::UnOptionalChaining, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |unresolved_mark| {
        UnOptionalChaining::new(unresolved_mark, level)
    })
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
fn does_not_recover_nested_optional_call_with_shadowed_undefined() {
    let input = r#"
function f(undefined) {
  (_a = (_b = runtime?.plugin) === null || _b === undefined ? undefined : _b.createHook) === null || _a === void 0 ? void 0 : _a.call(_b, "payload");
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
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
fn standard_transforms_babel_flattened_strict_optional_member_chain() {
    let input = r#"
var _r$foo$bar, _r;
const a = (_r = r) === null || _r === void 0 || (_r = _r.foo) === null || _r === void 0 || (_r = _r.bar) === null || _r === void 0 ? void 0 : _r.baz;
"#;
    let expected = r#"
var _r$foo$bar, _r;
const a = r?.foo?.bar?.baz;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_transform_babel_flattened_strict_optional_member_chain() {
    let input = r#"
var _r$foo$bar, _r;
const a = (_r = r) === null || _r === void 0 || (_r = _r.foo) === null || _r === void 0 || (_r = _r.bar) === null || _r === void 0 ? void 0 : _r.baz;
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn pipeline_transforms_babel_flattened_strict_optional_member_chain_with_nullish() {
    let input = r#"
var _r$foo$bar, _r;
const a = (_r$foo$bar = (_r = r) === null || _r === void 0 || (_r = _r.foo) === null || _r === void 0 || (_r = _r.bar) === null || _r === void 0 ? void 0 : _r.baz) !== null && _r$foo$bar !== void 0 ? _r$foo$bar : "fallback";
"#;
    let expected = r#"
let _r$foo$bar;
let _r;
const a = r?.foo?.bar?.baz ?? "fallback";
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn pipeline_transforms_esbuild_nested_optional_chain_with_nullish() {
    let input = r#"
var _a, _b, _c;
var x = (_c = (_b = (_a = value == null ? void 0 : value.foo) == null ? void 0 : _a.bar) == null ? void 0 : _b.baz) != null ? _c : "fallback";
"#;
    let expected = r#"
let _a;
let _b;
let _c;
const x = value?.foo?.bar?.baz ?? "fallback";
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_flattened_loose_optional_member_chain() {
    let input = r#"
var _r$foo$bar, _r;
const a = (_r = r) == null || (_r = _r.foo) == null || (_r = _r.bar) == null ? void 0 : _r.baz;
"#;
    let expected = r#"
var _r$foo$bar, _r;
const a = r?.foo?.bar?.baz;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_flattened_optional_call_with_memoized_context() {
    let input = r#"
var _m, _o;
const x = (_o = obj) === null || _o === void 0 || (_m = _o.method) === null || _m === void 0 ? void 0 : _m.call(_o, arg);
"#;
    let expected = r#"
var _m, _o;
const x = obj?.method?.(arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_flattened_optional_call_with_nested_memoized_context() {
    let input = r#"
var _m, _p, _o;
const x = (_o = obj) === null || _o === void 0 || (_p = _o.foo) === null || _p === void 0 || (_m = _p.method) === null || _m === void 0 ? void 0 : _m.call(_p, arg);
"#;
    let expected = r#"
var _m, _p, _o;
const x = obj?.foo?.method?.(arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_flattened_loose_optional_call_with_nested_memoized_context() {
    let input = r#"
var _m, _p, _o;
const x = (_o = obj) == null || (_p = _o.foo) == null || (_m = _p.method) == null ? void 0 : _m.call(_p, arg);
"#;
    let expected = r#"
var _m, _p, _o;
const x = obj?.foo?.method?.(arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_transform_babel_loose_optional_call_with_repeated_property() {
    let input = r#"
var _obj;
const out = (_obj = obj) == null || _obj.method == null ? void 0 : _obj.method(arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_transforms_babel_loose_optional_call_with_repeated_property() {
    let input = r#"
var _obj;
const out = (_obj = obj) == null || _obj.method == null ? void 0 : _obj.method(arg);
"#;
    let expected = r#"
var _obj;
const out = obj?.method?.(arg);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_transforms_babel_old_loose_optional_call_with_repeated_property() {
    let input = r#"
var _obj;
const out = (_obj = obj) == null ? void 0 : _obj.method == null ? void 0 : _obj.method(arg);
"#;
    let expected = r#"
var _obj;
const out = obj?.method?.(arg);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_transforms_babel_loose_nested_optional_call_with_repeated_property() {
    let input = r#"
var _obj;
const out = (_obj = obj) == null || (_obj = _obj.foo) == null || _obj.method == null ? void 0 : _obj.method(arg);
"#;
    let expected = r#"
var _obj;
const out = obj?.foo?.method?.(arg);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_transforms_babel_old_loose_nested_optional_call_with_repeated_property() {
    let input = r#"
var _obj, _obj_foo;
const out = (_obj = obj) == null ? void 0 : (_obj_foo = _obj.foo) == null ? void 0 : _obj_foo.method == null ? void 0 : _obj_foo.method(arg);
"#;
    let expected = r#"
var _obj, _obj_foo;
const out = obj?.foo?.method?.(arg);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_babel_flattened_optional_call_with_wrong_nested_context() {
    let input = r#"
var _m, _p, _o;
const x = (_o = obj) === null || _o === void 0 || (_p = _o.foo) === null || _p === void 0 || (_m = _p.method) === null || _m === void 0 ? void 0 : _m.call(_o, arg);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_preserves_babel_flattened_temp_when_observed_later() {
    let input = r#"
var _r;
const a = (_r = r) === null || _r === void 0 || (_r = _r.foo) === null || _r === void 0 ? void 0 : _r.bar;
use(_r);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_transforms_swc_nested_optional_member_chain() {
    let input = r#"
var _r_foo_bar, _r_foo, _r;
const a = (_r = r) === null || _r === void 0 ? void 0 : (_r_foo = _r.foo) === null || _r_foo === void 0 ? void 0 : (_r_foo_bar = _r_foo.bar) === null || _r_foo_bar === void 0 ? void 0 : _r_foo_bar.baz;
"#;
    let expected = r#"
var _r_foo_bar, _r_foo, _r;
const a = r?.foo?.bar?.baz;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_mixed_required_member_chain() {
    let input = r#"
var _obj_foo;
const out = (_obj_foo = obj.foo) === null || _obj_foo === void 0 || (_obj_foo = _obj_foo.bar.baz) === null || _obj_foo === void 0 ? void 0 : _obj_foo.qux;
"#;
    let expected = r#"
var _obj_foo;
const out = obj.foo?.bar.baz?.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_tsc_mixed_required_member_chain() {
    let input = r#"
var _a, _b;
const out = (_b = (_a = obj.foo) === null || _a === void 0 ? void 0 : _a.bar.baz) === null || _b === void 0 ? void 0 : _b.qux;
"#;
    let expected = r#"
var _a, _b;
const out = obj.foo?.bar.baz?.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_esbuild_mixed_required_member_chain() {
    let input = r#"
var _a, _b;
const out = (_b = (_a = obj.foo) == null ? void 0 : _a.bar.baz) == null ? void 0 : _b.qux;
"#;
    let expected = r#"
var _a, _b;
const out = obj.foo?.bar.baz?.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_old_loose_nested_mixed_required_member_chain() {
    let input = r#"
var _obj_foo, _obj_foo_bar_baz;
const out = (_obj_foo = obj.foo) == null ? void 0 : (_obj_foo_bar_baz = _obj_foo.bar.baz) == null ? void 0 : _obj_foo_bar_baz.qux;
"#;
    let expected = r#"
var _obj_foo, _obj_foo_bar_baz;
const out = obj.foo?.bar.baz?.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_mixed_leading_optional_member_chain() {
    let input = r#"
var _a;
const out = (_a = obj === null || obj === void 0 ? void 0 : obj.foo.bar) === null || _a === void 0 ? void 0 : _a.baz.qux;
"#;
    let expected = r#"
var _a;
const out = obj?.foo.bar?.baz.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_old_loose_nested_mixed_leading_optional_member_chain() {
    let input = r#"
var _obj, _obj_foo_bar;
const out = (_obj = obj) == null ? void 0 : (_obj_foo_bar = _obj.foo.bar) == null ? void 0 : _obj_foo_bar.baz.qux;
"#;
    let expected = r#"
var _obj, _obj_foo_bar;
const out = obj?.foo.bar?.baz.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_esbuild_mixed_leading_optional_member_chain() {
    let input = r#"
var _a;
const out = (_a = obj == null ? void 0 : obj.foo.bar) == null ? void 0 : _a.baz.qux;
"#;
    let expected = r#"
var _a;
const out = obj?.foo.bar?.baz.qux;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_babel_old_loose_computed_member_chain_with_nullish() {
    let input = r#"
var _obj_key_value, _obj, _obj_key;
const out = (_obj_key_value = (_obj = obj) == null ? void 0 : (_obj_key = _obj[key]) == null ? void 0 : _obj_key.value) != null ? _obj_key_value : fallback;
"#;
    let expected = r#"
let _obj_key_value;
let _obj;
let _obj_key;
const out = obj?.[key]?.value ?? fallback;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_transforms_swc_nested_optional_call_chain() {
    let input = r#"
var _obj_foo_method, _obj_foo, _obj;
const out = (_obj = obj) === null || _obj === void 0 ? void 0 : (_obj_foo = _obj.foo) === null || _obj_foo === void 0 ? void 0 : (_obj_foo_method = _obj_foo.method) === null || _obj_foo_method === void 0 ? void 0 : _obj_foo_method.call(_obj_foo, arg);
"#;
    let expected = r#"
var _obj_foo_method, _obj_foo, _obj;
const out = obj?.foo?.method?.(arg);
"#;
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
fn standard_transforms_declared_loose_eq_babel_assignment_member_access() {
    let input = r#"
var _;
const x = (_ = K.unhoistableHeaders) == null ? undefined : _.has(A);
"#;
    let expected = r#"
var _;
const x = K.unhoistableHeaders?.has(A);
"#;
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
