mod common;

use common::{assert_eq_normalized, render};

#[test]
fn then_null_handler_becomes_catch() {
    let input = r#"const x = promise.then(null, (e) => console.error(e))"#;
    let expected = r#"const x = promise.catch((e) => console.error(e))"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn then_undefined_handler_becomes_catch() {
    let input = r#"const x = promise.then(undefined, (e) => console.error(e))"#;
    let expected = r#"const x = promise.catch((e) => console.error(e))"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn then_void_0_handler_becomes_catch() {
    let input = r#"const x = promise.then(void 0, (e) => console.error(e))"#;
    let expected = r#"const x = promise.catch((e) => console.error(e))"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn then_with_both_handlers_unchanged() {
    let input = r#"const x = promise.then((v) => v, (e) => console.error(e))"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn then_with_one_arg_unchanged() {
    let input = r#"const x = promise.then((v) => v)"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn chained_then_null_becomes_catch() {
    let input = r#"const x = fetch(url).then(null, (e) => { throw e })"#;
    let expected = r#"const x = fetch(url).catch((e) => { throw e })"#;
    assert_eq_normalized(&render(input), expected);
}
