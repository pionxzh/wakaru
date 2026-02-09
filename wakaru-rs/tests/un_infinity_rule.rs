mod common;

use common::render;

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
    let output = render(input);
    let compact = output.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    assert!(compact.contains("0/0;Infinity;-Infinity;99/0;"));
    assert!(compact.contains("'0'/0;'1'/0;'-1'/0;'99'/0;"));
    assert!(compact.contains("x/0;"));
    assert!(compact.contains("[0/0,Infinity];"));
}
