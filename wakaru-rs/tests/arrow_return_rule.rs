mod common;

use wakaru_rs::rules::ArrowReturn;
use common::{assert_eq_normalized, render_pipeline, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| ArrowReturn)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn single_return_block_becomes_implicit_return() {
    let input = r#"
const double = x => {
    return x * 2;
};
"#;
    let expected = r#"
const double = x => x * 2;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_literal_return_becomes_parenthesized_expression_body() {
    let input = r#"
const build = () => {
    return { value: 1 };
};
"#;
    let expected = r#"
const build = () => ({
    value: 1
});
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn multiple_statements_keep_block_body() {
    let input = r#"
const logAndReturn = x => {
    console.log(x);
    return x;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn bare_return_keeps_block_body() {
    let input = r#"
const noop = () => {
    return;
};
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn pipeline_simplifies_new_arrows_created_by_arrow_function() {
    let input = r#"
const double = function(x) { return x * 2; };
"#;
    let expected = r#"
const double = x => x * 2;
"#;
    assert_eq_normalized(&apply_pipeline(input), expected);
}
