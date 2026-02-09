mod common;

use common::{assert_normalized_eq, normalize, render};

#[test]
fn splits_top_level_sequence_expression_statement() {
    // Reused from packages/unminify/src/transformations/__tests__/un-sequence-expression.spec.ts
    let input = r#"
a(), b(), c()
"#;
    let expected = r#"
a();
b();
c();
"#;

    let output = render(input);
    assert_normalized_eq(&output, expected);
}

#[test]
fn does_not_split_while_condition_but_splits_body_sequence_statement() {
    // Reused from packages/unminify/src/transformations/__tests__/un-sequence-expression.spec.ts
    let input = r#"
while (a(), b(), c()) {
  d(), e()
}
"#;
    let output = render(input);
    let compact = output.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    assert!(compact.contains("while(a(),b(),c()){d();e();}"));
}

#[test]
fn splits_return_sequence_expression() {
    // Reused from packages/unminify/src/transformations/__tests__/un-sequence-expression.spec.ts
    let input = r#"
if(a) return b(), c();
else return d = 1, e = 2, f = 3;
"#;

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("if (a) { b(); return c(); }"));
    assert!(normalized.contains("else { d = 1; e = 2; return f = 3; }"));
}

#[test]
fn splits_switch_discriminant_sequence_expression() {
    // Reused from packages/unminify/src/transformations/__tests__/un-sequence-expression.spec.ts
    let input = r#"
switch (a(), b(), c()) {
  case 1:
    d(), e()
}
"#;

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("a(); b(); switch(c())"));
    assert!(normalized.contains("d(); e();"));
}

#[test]
fn splits_throw_sequence_expression() {
    // Reused from packages/unminify/src/transformations/__tests__/un-sequence-expression.spec.ts
    let input = r#"
if(e !== null) throw a(), e
"#;

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("if (e !== null) { a(); throw e; }"));
}
