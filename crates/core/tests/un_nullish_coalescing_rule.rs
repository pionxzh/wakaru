mod common;

use common::{assert_eq_normalized, render_rule};
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
fn transforms_not_null_and_not_undefined_ternary() {
    let input = r#"foo !== null && foo !== void 0 ? foo : "bar""#;
    let expected = r#"foo ?? "bar""#;
    let output = apply(input);
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
    let expected = r#"var _ref;
foo ?? "bar""#;
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
    let expected = r#"var G;
B.broadcast ?? true"#;
    let output = apply(input);
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
    let expected = r#"var G;
B?.broadcast ?? true"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_negated_or_chain() {
    // Negated form: !((tmp = obj?.prop) === null || tmp === undefined || tmp)
    let input = r#"var G;
!((G = B?.redirect) === null || G === undefined || G)"#;
    let expected = r#"var G;
!(B?.redirect ?? true)"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_or_chain_flipped_comparisons() {
    // Exact shape from issue-52 after FlipComparisons: null on the left
    let input = r#"var G;
null === (G = null == B ? void 0 : B.broadcast) || void 0 === G || G"#;
    let expected = r#"var G;
(null == B ? void 0 : B.broadcast) ?? true"#;
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
