mod common;

use common::{assert_compact_eq, render};

#[test]
fn transforms_bracket_notation_to_dot_or_numeric() {
    // Reused from packages/unminify/src/transformations/__tests__/un-bracket-notation.spec.ts
    let input = r#"
obj['bar'];
obj['bar'].baz;
obj['bar']['baz'];
obj['bar'].baz['qux'];

obj['1'];
obj['0'];
obj['00'];
obj['3.14'];
"#;
    let expected = r#"
obj.bar;
obj.bar.baz;
obj.bar.baz;
obj.bar.baz.qux;

obj[1];
obj[0];
obj['00'];
obj[3.14];
"#;

    let output = render(input);
    assert_compact_eq(&output, expected);
}

#[test]
fn keeps_invalid_or_reserved_bracket_notation() {
    // Reused from packages/unminify/src/transformations/__tests__/un-bracket-notation.spec.ts
    let input = r#"
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
"#;
    let expected = r#"
obj[a];
obj[''];
obj[' '];
obj['var'];
obj['let'];
obj['const'];
obj['await'];
obj['1var'];
obj['prop-with-dash'];
"#;

    let output = render(input);
    assert_compact_eq(&output, expected);
}
