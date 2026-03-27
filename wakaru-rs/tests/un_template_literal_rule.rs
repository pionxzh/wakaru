mod common;

use common::{assert_eq_normalized, render};

#[test]
fn restores_template_literal_from_concat_chain() {
    // Reused from packages/unminify/src/transformations/__tests__/un-template-literal.spec.ts
    let input = r#"
var example1 = "the ".concat("simple ", form);
var example2 = "".concat(1);
var example3 = 1 + "".concat(foo).concat(bar).concat(baz);
var example4 = 1 + "".concat(foo, "bar").concat(baz);
var example5 = "".concat(1, f, "oo", true).concat(b, "ar", 0).concat(baz);
var example6 = "test ".concat(foo, " ").concat(bar);
"#;
    // VarDeclToLetConst converts var to const since these vars are never reassigned.
    let expected = r#"
const example1 = `the simple ${form}`;
const example2 = `${1}`;
const example3 = 1 + `${foo}${bar}${baz}`;
const example4 = 1 + `${foo}bar${baz}`;
const example5 = `${1}${f}oo${true}${b}ar${0}${baz}`;
const example6 = `test ${foo} ${bar}`;
"#;

    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_non_consecutive_concat_calls() {
    // Reused from packages/unminify/src/transformations/__tests__/un-template-literal.spec.ts
    let input = r#"
"the".concat(first, " take the ").concat(second, " and ").split(' ').concat(third);
"#;
    let expected = r#"
`the${first} take the ${second} and `.split(' ').concat(third);
"#;

    let output = render(input);
    assert_eq_normalized(&output, expected);
}

