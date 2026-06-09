mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::facts::{HelperExportFact, HelperKind, ModuleFacts, ModuleFactsMap};
use wakaru_core::rules::{RewriteLevel, UnTemplateLiteral};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnTemplateLiteral::new())
}

fn apply_minimal(input: &str) -> String {
    render_rule(input, |_| {
        UnTemplateLiteral::new_with_level(RewriteLevel::Minimal)
    })
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
fn keeps_plus_empty_string_conversion() {
    let input = r#"
var actual = object + "";
var other = "" + object;
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_rewrites_empty_string_conversion() {
    let input = r#"
var actual = object + "";
var other = "" + object;
"#;
    let expected = r#"
var actual = `${object}`;
var other = `${object}`;
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
    let expected = "var msg = `redux-saga ${level}: ${text}\n${extra}`;\n";
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
fn standard_normalizes_escaped_newlines_in_untagged_template() {
    let input = r#"var slideIn = (direction) => `\n0% {transform: translate3d(0, ${-200 * direction}%, 0)}\n100% {transform: translate3d(0, 0, 0)}\n`;"#;
    let expected = r#"var slideIn = (direction)=>`
0% {transform: translate3d(0, ${-200 * direction}%, 0)}
100% {transform: translate3d(0, 0, 0)}
`;
"#;

    let output = apply(input);
    assert_eq!(output, expected);
}

#[test]
fn minimal_preserves_escaped_newlines_in_untagged_template() {
    let input = r#"var slideIn = (direction) => `\n0% {transform: translate3d(0, ${-200 * direction}%, 0)}\n100% {transform: translate3d(0, 0, 0)}\n`;"#;
    let expected = r#"var slideIn = (direction)=>`\n0% {transform: translate3d(0, ${-200 * direction}%, 0)}\n100% {transform: translate3d(0, 0, 0)}\n`;
"#;

    let output = apply_minimal(input);
    assert_eq!(output, expected);
}

#[test]
fn keeps_escaped_newlines_in_tagged_template_raw() {
    let input = r#"var out = tag`\n0% {transform}\n`;"#;
    let expected = r#"var out = tag`\n0% {transform}\n`;
"#;

    let output = apply(input);
    assert_eq!(output, expected);
}

#[test]
fn keeps_literal_backslash_n_in_untagged_template() {
    let input = r#"var out = `\\nnot a line break`;"#;
    let expected = r#"var out = `\\nnot a line break`;
"#;

    let output = apply(input);
    assert_eq!(output, expected);
}

#[test]
fn keeps_escaped_crlf_in_untagged_template() {
    let input = r#"var out = `header\r\n\r\n`;"#;
    let expected = r#"var out = `header\r\n\r\n`;
"#;

    let output = apply(input);
    assert_eq!(output, expected);
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
fn restores_swc_tagged_template_newlines_without_raw_argument() {
    let input = r#"
function _tagged_template_literal(strings, raw) { return strings; }
function _templateObject() {
    var data = _tagged_template_literal([
        "\n  staticOne\n  staticTwo\n  ",
        "\n  ",
        "\n  staticThree\n  ",
        "\n"
    ]);
    _templateObject = function _templateObject() {
        return data;
    };
    return data;
}
var out = tag(_templateObject(), dynamicOne, dynamicTwo, dynamicThree);
"#;
    let expected = r#"var out = tag`
  staticOne
  staticTwo
  ${dynamicOne}
  ${dynamicTwo}
  staticThree
  ${dynamicThree}
`;
"#;

    let output = apply(input);
    assert_eq!(output, expected);
}

#[test]
fn restores_cross_module_default_object_swc_tagged_template() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            default_object_helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("template".into()),
                kind: HelperKind::TaggedTemplateLiteral,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import helpers from "./helpers.js";
function _templateObject() {
    const data = helpers._(["hello ", ""]);
    _templateObject = () => data;
    return data;
}
var out = tag(_templateObject(), name);
"#;
    let expected = r#"
import helpers from "./helpers.js";
var out = tag`hello ${name}`;
"#;
    let output = render_rule(input, |_| {
        UnTemplateLiteral::new_with_facts(RewriteLevel::Standard, &facts)
    });
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_cross_module_direct_tagged_template_keeps_import() {
    let mut facts = ModuleFactsMap::new();
    facts.insert(
        "helpers.js",
        ModuleFacts {
            helper_exports: vec![HelperExportFact {
                exported: "_".into(),
                local: Some("template".into()),
                kind: HelperKind::TaggedTemplateLiteral,
            }],
            ..Default::default()
        },
    );

    let input = r#"
import { _ as template } from "./helpers.js";
var out = tag(template(["hello ", ""], ["hello ", ""]), name);
"#;
    let expected = r#"
import { _ as template } from "./helpers.js";
var out = tag`hello ${name}`;
"#;
    let output = render_rule(input, |_| {
        UnTemplateLiteral::new_with_facts(RewriteLevel::Standard, &facts)
    });
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
fn restores_esbuild_terser_inlined_cached_tagged_template() {
    let input = r#"
var _a;
var out = tag(_a || (_a = function(cooked, raw) { return cooked; }(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_esbuild_terser_inlined_raw_tagged_template() {
    let input = r#"
var _a;
var out = tag(_a || (_a = function(cooked, raw) { return cooked; }(["line\n", "😀"], ["line\\n", "\\u{1f600}"])), value);
"#;
    let expected = r#"
var out = tag`line\n${value}\u{1f600}`;
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

#[test]
fn removes_consumed_template_cache_from_shared_var_decl() {
    let input = r#"
var _templateObject, keep = 1;
function _taggedTemplateLiteral(e, t) { return e; }
var out = tag(_templateObject || (_templateObject = _taggedTemplateLiteral(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var keep = 1;
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
