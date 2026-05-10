mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnWhileLoop;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnWhileLoop)
}

#[test]
fn transforms_infinite_for_to_while_true() {
    // Reused from packages/unminify/src/transformations/__tests__/un-while-loop.spec.ts
    let input = r#"
for (;;) {
    doSomething();
}
"#;
    let expected = r#"
while (true) {
    doSomething();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_for_with_condition_only_to_while() {
    // Reused from packages/unminify/src/transformations/__tests__/un-while-loop.spec.ts
    let input = r#"
for (; i < 10;) {
    i++;
}
"#;
    let expected = r#"
while (i < 10) {
    i++;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_for_with_init() {
    // Reused from packages/unminify/src/transformations/__tests__/un-while-loop.spec.ts
    let input = r#"
for (let i = 0;;) {
    doSomething();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn does_not_transform_for_with_update() {
    // Reused from packages/unminify/src/transformations/__tests__/un-while-loop.spec.ts
    let input = r#"
for (;; i++) {
    doSomething();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}
