mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnminifyBooleans;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnminifyBooleans)
}

#[test]
fn transforms_bang_zero_and_bang_one() {
    // Reused from packages/unminify/src/transformations/__tests__/un-boolean.spec.ts
    let input = r#"
let a = !1;
const b = !0;

var obj = {
  value: !0
};
"#;
    let expected = r#"
let a = false;
const b = true;

var obj = {
  value: true
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
