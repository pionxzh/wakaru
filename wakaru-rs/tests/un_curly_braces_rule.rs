mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnCurlyBraces;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnCurlyBraces)
}

#[test]
fn wraps_if_body_in_block() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
if (a) b();
"#;
    let expected = r#"
if (a) {
    b();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn wraps_if_else_bodies_in_blocks() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
if (a) b();
else c();
"#;
    let expected = r#"
if (a) {
    b();
} else {
    c();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_else_if_chain_without_extra_wrapping() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
if (a) b();
else if (c) d();
else e();
"#;
    let expected = r#"
if (a) {
    b();
} else if (c) {
    d();
} else {
    e();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn wraps_for_loop_body_in_block() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
for (let i = 0; i < 10; i++) doSomething(i);
"#;
    let expected = r#"
for (let i = 0; i < 10; i++) {
    doSomething(i);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn wraps_while_loop_body_in_block() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
while (x) doSomething();
"#;
    let expected = r#"
while (x) {
    doSomething();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn wraps_do_while_body_in_block() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
do doSomething(); while (x);
"#;
    let expected = r#"
do {
    doSomething();
} while (x);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn wraps_arrow_function_expr_body_in_block_with_return() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    // Note: UnCurlyBraces wraps the expression body in a block, but ArrowFunction
    // (which runs after) simplifies { return b(); } back to expression body b().
    let input = r#"
const fn = () => b();
"#;
    let expected = r#"
const fn = () => {
    return b();
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_wrap_var_declaration_in_if_body() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
if (a) var b = 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_double_wrap_existing_block() {
    // Reused from packages/unminify/src/transformations/__tests__/un-curly-braces.spec.ts
    let input = r#"
if (a) {
    b();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
