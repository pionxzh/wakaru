mod common;

use common::normalize;
use common::render;

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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("obj.bar;"));
    assert!(normalized.contains("obj.bar.baz;"));
    assert!(normalized.contains("obj.bar.baz.qux;"));
    assert!(normalized.contains("obj[1];"));
    assert!(normalized.contains("obj[0];"));
    assert!(normalized.contains("obj['00'];"));
    assert!(normalized.contains("obj[3.14];"));
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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("obj[a];"));
    assert!(normalized.contains("obj[''];"));
    assert!(normalized.contains("obj[' '];"));
    assert!(normalized.contains("obj['var'];"));
    assert!(normalized.contains("obj['let'];"));
    assert!(normalized.contains("obj['const'];"));
    assert!(normalized.contains("obj['await'];"));
    assert!(normalized.contains("obj['1var'];"));
    assert!(normalized.contains("obj['prop-with-dash'];"));
}
