mod common;

use common::{assert_eq_normalized, render};

#[test]
fn transforms_one_div_zero_to_infinity() {
    // Reused from packages/unminify/src/transformations/__tests__/un-infinity.spec.ts
    let input = r#"
0 / 0;
1 / 0;
-1 / 0;
99 / 0;

'0' / 0;
'1' / 0;
'-1' / 0;
'99' / 0;

x / 0;

[0 / 0, 1 / 0]
"#;
    let expected = r#"
0 / 0;
Infinity;
-Infinity;
99 / 0;

'0' / 0;
'1' / 0;
'-1' / 0;
'99' / 0;

x / 0;

    [0 / 0, Infinity];
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
