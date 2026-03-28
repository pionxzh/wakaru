mod common;

use common::{assert_eq_normalized, render};

#[test]
fn math_pow_becomes_exponent() {
    let input = r#"const x = Math.pow(2, 10);"#;
    let expected = r#"const x = 2 ** 10;"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn math_pow_with_variables() {
    let input = r#"const x = Math.pow(a, b);"#;
    let expected = r#"const x = a ** b;"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn math_pow_with_expressions() {
    let input = r#"const x = Math.pow(a + 1, b * 2);"#;
    let expected = r#"const x = (a + 1) ** (b * 2);"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn nested_math_pow() {
    // ** is right-associative: Math.pow(Math.pow(2,3), 4) = (2**3)**4 which needs parens
    let input = r#"const x = Math.pow(Math.pow(2, 3), 4);"#;
    let expected = r#"const x = (2 ** 3) ** 4;"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn math_pow_one_arg_not_converted() {
    let input = r#"const x = Math.pow(2);"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn math_pow_three_args_not_converted() {
    let input = r#"const x = Math.pow(2, 3, 4);"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn math_pow_spread_arg_not_converted() {
    let input = r#"const x = Math.pow(...args);"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn other_math_methods_not_converted() {
    let input = r#"const x = Math.sqrt(4);"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn non_math_pow_not_converted() {
    let input = r#"const x = foo.pow(2, 3);"#;
    assert_eq_normalized(&render(input), input);
}
