mod common;

use common::{assert_eq_normalized, render};

#[test]
fn typeof_loose_eq_becomes_strict() {
    let input = r#"const x = typeof e == "function""#;
    let expected = r#"const x = typeof e === "function""#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn typeof_loose_neq_becomes_strict() {
    let input = r#"const x = typeof e != "function""#;
    let expected = r#"const x = typeof e !== "function""#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn typeof_strict_eq_unchanged() {
    let input = r#"const x = typeof e === "string""#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn typeof_on_right_side() {
    // After FlipComparisons: "string" == typeof e → typeof e == "string"
    // Then this rule: typeof e == "string" → typeof e === "string"
    let input = r#"const x = "string" == typeof e"#;
    let expected = r#"const x = typeof e === "string""#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn typeof_eq_object() {
    let input = r#"const x = typeof e == "object""#;
    let expected = r#"const x = typeof e === "object""#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn typeof_neq_undefined() {
    let input = r#"const x = typeof e != "undefined""#;
    let expected = r#"const x = typeof e !== "undefined""#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn non_typeof_loose_eq_unchanged() {
    // Regular == should NOT be upgraded (not safe in general)
    let input = r#"const x = a == "hello""#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn typeof_eq_non_string_unchanged() {
    // typeof compared to non-string — don't touch
    let input = r#"const x = typeof e == 42"#;
    assert_eq_normalized(&render(input), input);
}
