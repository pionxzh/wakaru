mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnNumericLiteral;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnNumericLiteral)
}

#[test]
fn transforms_numeric_literals_with_different_notation() {
    // Reused from packages/unminify/src/transformations/__tests__/un-numeric-literal.spec.ts
    // Using variable declarations so literals are in a meaningful context
    let input = r#"
const a = 65536;
const b = 123.4;
const c = 0b101010;
const d = 0o777;
const e = -0x123;
const f = 4.2e2;
const g = -2e4;
"#;
    let expected = r#"
const a = 65536;
const b = 123.4;
const c = 42;
const d = 511;
const e = -291;
const f = 420;
const g = -20000;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
