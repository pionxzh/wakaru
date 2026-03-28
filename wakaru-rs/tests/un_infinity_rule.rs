mod common;

use wakaru_rs::rules::UnInfinity;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnInfinity)
}

#[test]
fn transforms_one_div_zero_to_infinity() {
    // Reused from packages/unminify/src/transformations/__tests__/un-infinity.spec.ts
    let input = r#"
const a = 1 / 0;
const b = -1 / 0;
const c = 0 / 0;
const d = 99 / 0;
const e = '1' / 0;
const f = x / 0;
const g = [0 / 0, 1 / 0];
"#;
    let expected = r#"
const a = Infinity;
const b = -Infinity;
const c = 0 / 0;
const d = 99 / 0;
const e = '1' / 0;
const f = x / 0;
const g = [0 / 0, Infinity];
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}


