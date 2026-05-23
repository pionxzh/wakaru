mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnTemplateLiteral;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnTemplateLiteral)
}

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
var example1 = `the simple ${form}`;
var example2 = `${1}`;
var example3 = 1 + `${foo}${bar}${baz}`;
var example4 = 1 + `${foo}bar${baz}`;
var example5 = `${1}${f}oo${true}${b}ar${0}${baz}`;
var example6 = `test ${foo} ${bar}`;
"#;

    let output = apply(input);
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

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn plus_chain_starting_with_string_literal() {
    let input = r#"
var a = "prefix: " + value;
var b = "hello, " + name + "!";
var c = "@@redux-saga/" + key;
"#;
    let expected = r#"
var a = `prefix: ${value}`;
var b = `hello, ${name}!`;
var c = `@@redux-saga/${key}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn plus_chain_ending_with_string_literal() {
    let input = r#"
var a = value + " suffix";
var b = expr + " has been deprecated";
"#;
    let expected = r#"
var a = `${value} suffix`;
var b = `${expr} has been deprecated`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn plus_chain_groups_non_string_prefix() {
    // `a + b + "c"` must NOT become `${a}${b}c` (breaks arithmetic for numbers).
    // The non-string prefix `a + b` is kept as a single grouped expression.
    let input = r#"
var result = prefix + count + " items";
"#;
    let expected = r#"
var result = `${prefix + count} items`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn plus_chain_mixed_string_positions() {
    let input = r#"
var msg = "redux-saga " + level + ": " + text + "\n" + extra;
"#;
    let expected = r#"
var msg = `redux-saga ${level}: ${text}\n${extra}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_plus_chain_inside_template_expression() {
    // Inner concatenation inside a logical expression should also be converted.
    // Previously required a double-pass because converting the outer chain
    // returned early without visiting children of the new template literal.
    let input = r#"
var msg = "Given " + (n && 'action "' + String(n) + '"' || "an action") + ', reducer "' + e + '" returned undefined.';
"#;
    let expected = r#"
var msg = `Given ${n && `action "${String(n)}"` || "an action"}, reducer "${e}" returned undefined.`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn pure_number_addition_not_transformed() {
    // No string literals → must not be turned into a template.
    let input = r#"
var x = a + b + c;
var y = 1 + 2;
"#;
    let expected = r#"
var x = a + b + c;
var y = 1 + 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_babel_modern_tagged_template() {
    let input = r#"
var _templateObject;
function _taggedTemplateLiteral(e, t) { return e; }
var out = tag(_templateObject || (_templateObject = _taggedTemplateLiteral(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_typescript_tagged_template() {
    let input = r#"
var __makeTemplateObject = (this && this.__makeTemplateObject) || function (cooked, raw) { return cooked; };
var out = tag(__makeTemplateObject(["hello ", ""], ["hello ", ""]), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_swc_tagged_template() {
    let input = r#"
function _tagged_template_literal(strings, raw) { return strings; }
var out = tag(_tagged_template_literal(["hello ", ""], ["hello ", ""]), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_esbuild_cached_tagged_template() {
    let input = r#"
var __template = function(cooked, raw) { return cooked; };
var _a;
var out = tag(_a || (_a = __template(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_babel_cache_function_tagged_template() {
    let input = r#"
function _templateObject() {
    const data = _taggedTemplateLiteral(["hello ", ""]);
    _templateObject = function () { return data; };
    return data;
}
function _taggedTemplateLiteral(e, t) { return e; }
var out = tag(_templateObject(), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_member_tagged_template_with_raw_segments() {
    let input = r#"
var _templateObject;
function _taggedTemplateLiteral(e, t) { return e; }
var out = css.div(_templateObject || (_templateObject = _taggedTemplateLiteral(["line\n", ""], ["line\\n", ""])), value);
"#;
    let expected = r#"
var out = css.div`line\n${value}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
