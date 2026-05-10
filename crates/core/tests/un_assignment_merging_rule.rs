mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::UnAssignmentMerging;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnAssignmentMerging)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn splits_two_level_chained_assignment() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    // UnAssignmentMerging splits into: exports.foo = 1; exports.bar = 1;
    // UnEsm then converts to ESM exports
    let input = r#"
exports.foo = exports.bar = 1;
"#;
    let expected = r#"
export const foo = 1;
export const bar = 1;
"#;

    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_three_level_chained_assignment() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = c = undefined;
"#;
    let expected = r#"
a = undefined;
b = undefined;
c = undefined;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_member_expression_final_value() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = foo.bar;
"#;
    let expected = r#"
a = b = foo.bar;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_call_expression_final_value() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = fn();
"#;
    let expected = r#"
a = b = fn();
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
