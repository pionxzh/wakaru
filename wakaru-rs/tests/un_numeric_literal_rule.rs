mod common;

use common::assert_eq_normalized;
use common::render;

#[test]
fn transforms_numeric_literals_with_different_notation() {
    // Reused from packages/unminify/src/transformations/__tests__/un-numeric-literal.spec.ts
    let input = r#"
65536;
123.4;
0b101010;
0o777;
-0x123;
4.2e2;
-2e4;
"#;
    let expected = r#"
65536;
123.4;
42;
511;
-291;
420;
-20000;
"#;

    let output = render(input);
    assert_eq_normalized(&output, expected);
}

