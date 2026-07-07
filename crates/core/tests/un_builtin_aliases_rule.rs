mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnBuiltinAliases;

fn apply(input: &str) -> String {
    render_rule(input, UnBuiltinAliases::new)
}

#[test]
fn inlines_module_var_builtin_member_aliases() {
    let input = r#"
var e = Object.freeze;
var r = Object.defineProperty;
use(e(r(strings, "raw", { value: e(raws) })));
"#;
    let expected = r#"
use(Object.freeze(Object.defineProperty(strings, "raw", {
    value: Object.freeze(raws)
})));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inlines_module_const_builtin_member_aliases() {
    let input = r#"
const e = Object.freeze;
use(e(value));
"#;
    let expected = r#"
use(Object.freeze(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_var_alias_used_before_initializer() {
    let input = r#"
use(e);
var e = Object.freeze;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_reassigned_var_alias() {
    let input = r#"
var e = Object.freeze;
e = other;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_when_direct_eval_can_observe_binding() {
    let input = r#"
var e = Object.freeze;
eval("e");
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_alias_from_local_builtin_shadow() {
    let input = r#"
const Object = fake;
var e = Object.freeze;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_redeclared_with_non_alias_init() {
    let input = r#"
var e = Object.freeze;
use(e(value));
var e = getPolyfill();
use2(e);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_redeclaring_non_alias_binding() {
    let input = r#"
var e = getPolyfill();
use(e);
var e = Object.freeze;
use2(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_mutated_by_update_expression() {
    let input = r#"
var e = Object.freeze;
e++;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_removed_with_delete() {
    let input = r#"
var e = Object.freeze;
use(delete e);
use2(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
