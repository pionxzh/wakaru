mod common;

use common::{assert_eq_normalized, render};

#[test]
fn let_undefined_becomes_let() {
    let input = r#"let x = undefined"#;
    let expected = r#"let x"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn let_void_0_becomes_let() {
    let input = r#"let x = void 0"#;
    let expected = r#"let x"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn var_undefined_becomes_var() {
    let input = r#"var x = undefined"#;
    // var_decl_to_let_const will convert to let, but the undefined removal is the point
    let expected = r#"let x"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn const_undefined_unchanged() {
    // const must have an initializer
    let input = r#"const x = undefined"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn let_with_value_unchanged() {
    let input = r#"let x = 42"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn let_multiple_decls_partial() {
    // VarDeclToLetConst splits multi-decl, but undefined init is still removed
    let input = r#"let x = undefined, y = 42"#;
    let expected = "let x;\nlet y = 42;";
    assert_eq_normalized(&render(input), expected);
}
