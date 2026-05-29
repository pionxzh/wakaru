mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnUndefinedInit;

fn apply(input: &str) -> String {
    render_rule(input, UnUndefinedInit::new)
}

#[test]
fn let_undefined_becomes_let() {
    let input = r#"let x = undefined"#;
    let expected = r#"let x"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn let_void_0_becomes_let() {
    let input = r#"let x = void 0"#;
    let expected = r#"let x"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_undefined_becomes_var() {
    let input = r#"var x = undefined"#;
    let expected = r#"var x"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn const_undefined_unchanged() {
    // const must have an initializer
    let input = r#"const x = undefined"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn let_with_value_unchanged() {
    let input = r#"let x = 42"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn let_multiple_decls_partial() {
    let input = r#"let x = undefined, y = 42"#;
    let expected = "let x, y = 42;";
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn object_destructuring_undefined_init_preserved() {
    let input = r#"let {} = undefined"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn array_destructuring_undefined_init_preserved() {
    let input = r#"let [] = undefined"#;
    assert_eq_normalized(&apply(input), input);
}
