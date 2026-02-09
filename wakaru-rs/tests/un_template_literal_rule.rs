mod common;

use common::normalize;
use common::render;

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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("var example1 = `the simple ${form}`;"));
    assert!(normalized.contains("var example2 = `${1}`;"));
    assert!(normalized.contains("var example3 = 1 + `${foo}${bar}${baz}`;"));
    assert!(normalized.contains("var example4 = 1 + `${foo}bar${baz}`;"));
    assert!(normalized.contains("var example5 = `${1}${f}oo${true}${b}ar${0}${baz}`;"));
    assert!(normalized.contains("var example6 = `test ${foo} ${bar}`;"));
}

#[test]
fn keeps_non_consecutive_concat_calls() {
    // Reused from packages/unminify/src/transformations/__tests__/un-template-literal.spec.ts
    let input = r#"
"the".concat(first, " take the ").concat(second, " and ").split(' ').concat(third);
"#;

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("`the${first} take the ${second} and `.split(' ').concat(third);"));
}
