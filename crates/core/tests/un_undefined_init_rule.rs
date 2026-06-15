mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnUndefinedInit;

fn apply(input: &str) -> String {
    render_rule(input, UnUndefinedInit::new)
}

#[test]
fn let_undefined_becomes_let() {
    let input = r#"let x = undefined"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn let_void_0_becomes_let() {
    let input = r#"let x = void 0"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn var_undefined_becomes_var() {
    let input = r#"var x = undefined"#;
    let expected = r#""#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn used_undefined_init_becomes_bare_decl() {
    let input = r#"
let x = undefined;
use(x);
"#;
    let expected = r#"
let x;
use(x);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn eval_observable_undefined_init_becomes_bare_decl() {
    let input = r#"
function f() {
  let x = undefined;
  eval("x");
}
"#;
    let expected = r#"
function f() {
  let x;
  eval("x");
}
"#;
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
    let expected = "let y = 42;";
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
