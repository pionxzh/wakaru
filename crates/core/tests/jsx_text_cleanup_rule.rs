mod common;

use common::render_rule;
use wakaru_core::rules::UnJsx;

fn apply(input: &str) -> String {
    render_rule(input, |mark| {
        UnJsx::new_with_level(mark, wakaru_core::RewriteLevel::Standard)
    })
}

#[test]
fn string_child_trailing_space_becomes_text() {
    let input = r#"React.createElement("h2", null, "Counter: ", count)"#;
    let output = apply(input);
    assert!(
        output.contains("Counter: {count}"),
        "Expected inline JSXText for trailing-space string, got: {}",
        output
    );
}

#[test]
fn string_child_leading_space_becomes_text() {
    let input = r#"React.createElement("span", null, " hello", name)"#;
    let output = apply(input);
    assert!(
        !output.contains(r#"{" hello"}"#),
        "Expected JSXText not expression container for leading-space string, got: {}",
        output
    );
}

#[test]
fn string_child_no_special_chars_becomes_text() {
    let input = r#"React.createElement("span", null, "Hello World")"#;
    let output = apply(input);
    assert!(
        output.contains(">Hello World<"),
        "Expected JSXText for simple string, got: {}",
        output
    );
}

#[test]
fn string_child_with_braces_stays_expr() {
    let input = r#"React.createElement("span", null, "x = {y}")"#;
    let output = apply(input);
    assert!(
        output.contains(r#"{"x = {y}"}"#),
        "Expected JSXExprContainer for string with braces, got: {}",
        output
    );
}

#[test]
fn string_child_with_angle_brackets_stays_expr() {
    let input = r#"React.createElement("span", null, "<div>")"#;
    let output = apply(input);
    assert!(
        output.contains(r#"{"<div>"}"#),
        "Expected JSXExprContainer for string with angle brackets, got: {}",
        output
    );
}

#[test]
fn string_child_with_newline_stays_expr() {
    let input = r#"React.createElement("pre", null, "line1\nline2")"#;
    let output = apply(input);
    assert!(
        output.contains("{\"line1\\nline2\"}"),
        "Expected JSXExprContainer for string with newline, got: {}",
        output
    );
}

#[test]
fn empty_string_child_stays_expr() {
    let input = r#"React.createElement("span", null, "")"#;
    let output = apply(input);
    assert!(
        output.contains(r#"{""}"#),
        "Expected JSXExprContainer for empty string, got: {}",
        output
    );
}

#[test]
fn mixed_text_and_expr_children() {
    let input = r#"React.createElement("p", null, "Hello ", name, "!")"#;
    let output = apply(input);
    assert!(
        output.contains("Hello {name}!"),
        "Expected inline text around expression, got: {}",
        output
    );
}

#[test]
fn switch_to_prefix_renders_inline() {
    let input = r#"React.createElement("button", null, "Switch to ", mode)"#;
    let output = apply(input);
    assert!(
        output.contains("Switch to {mode}"),
        "Expected 'Switch to' as inline text, got: {}",
        output
    );
}
