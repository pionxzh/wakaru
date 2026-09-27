mod common;

use common::{assert_eq_normalized, inspect_rule_output, render, render_rule};
use swc_core::ecma::ast::TplElement;
use swc_core::ecma::visit::{Visit, VisitWith};
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
fn restores_babel_runtime_imported_tagged_template() {
    // transform-runtime imports the helper from @babel/runtime.
    let input = r#"
import _taggedTemplateLiteral from "@babel/runtime/helpers/taggedTemplateLiteral";
var _templateObject;
var out = tag(_templateObject || (_templateObject = _taggedTemplateLiteral(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_aliased_babel_runtime_imported_tagged_template() {
    // The local name is not a known helper name — detection must go through
    // LocalHelperContext path classification, not is_template_helper_name.
    let input = r#"
import t from "@babel/runtime/helpers/taggedTemplateLiteral";
var _templateObject;
var out = tag(_templateObject || (_templateObject = t(["hello ", ""], ["hello ", ""])), name);
"#;
    let expected = r#"
var out = tag`hello ${name}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn restores_swc_imported_tagged_template_loose() {
    let input = r#"
import { _ as _tagged_template_literal_loose } from "@swc/helpers/_/_tagged_template_literal_loose";
var _templateObject;
var out = tag(_templateObject || (_templateObject = _tagged_template_literal_loose(["hello ", ""], ["hello ", ""])), name);
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
fn restores_esbuild_terser_mangled_member_tagged_template() {
    // Produced by esbuild ES5 output minified with Terser compress+mangle.
    let input = r#"
var e=Object.freeze,r=Object.defineProperty,c=function(c,o){return e(r(c,"raw",{value:e(o||c.slice())}))},o,a=css.div(o||(o=c(["color: ","; margin: ","px;"])),color,space);use(a);
"#;
    let expected = r#"
const a = css.div`color: ${color}; margin: ${space}px;`;
use(a);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn restores_mangled_babel_loose_tagged_template() {
    // babel-7.8-loose-terser-compress-mangle: the helper body is
    // `n.raw = r` with a `.slice(0)` copy — no Object.freeze.
    // Full pipeline: SimplifySequence normalizes the comma-return into
    // separate statements before helper detection runs.
    let input = r#"
function n(){const t=r(["hello ",""]);return n=function(){return t},t}
function r(n,r){return r||(r=n.slice(0)),n.raw=r,n}
var t=tag(n(),name);
"#;
    let expected = r#"
const t = tag`hello ${name}`;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn restores_mangled_tsc_tagged_template() {
    // tsc-es5-terser-compress-mangle: uses Object.defineProperty(e, "raw", ...)
    // instead of Object.freeze(Object.defineProperties(...)).
    let input = r#"
var e=this&&this.__makeTemplateObject||function(e,t){return Object.defineProperty?Object.defineProperty(e,"raw",{value:t}):e.raw=t,e},t=tag(e(["hello ",""],["hello ",""]),name);
"#;
    let expected = r#"
const t = tag`hello ${name}`;
"#;
    assert_eq_normalized(&render(input), expected);
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

// ── minimal: only syntax-proven primitive substitutions ──────────────────────

#[test]
fn minimal_keeps_plus_chain_with_unproven_substitutions() {
    // `+` coerces with hint "default", a template with hint "string"; an
    // identifier, member, or call may hold an object whose valueOf and
    // toString disagree. `standard` accepts this under `string_coercion_hint`;
    // `minimal` does not.
    let input = r#"
var a = "prefix: " + value;
var b = "user " + user.name + "!";
var c = "id=" + read();
var d = value + " suffix";
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn minimal_rewrites_plus_chain_with_syntax_proven_primitive_substitutions() {
    // Literals, arithmetic, typeof, and nested templates are primitives by
    // syntax; ToPrimitive is the identity on them, so the rewrite is exact.
    let input = r#"
var a = "n=" + 1 + " ok=" + true;
var b = "sum=" + (x + y) + " type=" + typeof z;
var c = "neg=" + -count + " inner=" + `t${1}`;
var d = "pick=" + (flag ? 1 : "no") + " cmp=" + (a < b);
"#;
    let expected = r#"
var a = `n=${1} ok=${true}`;
var b = `sum=${x + y} type=${typeof z}`;
var c = `neg=${-count} inner=${`t${1}`}`;
var d = `pick=${flag ? 1 : "no"} cmp=${a < b}`;
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_keeps_logical_substitutions_with_unproven_operands() {
    // `a || b` yields one operand unchanged, so it is primitive only when both
    // operands are.
    let input = r#"
var a = "v=" + (value || fallback);
var b = "v=" + (1 || "x");
"#;
    let expected = r#"
var a = "v=" + (value || fallback);
var b = `v=${1 || "x"}`;
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_keeps_concat_chain_with_unproven_substitutions() {
    // concat evaluates every argument before coercing any; a template
    // interleaves the two. `standard` accepts this under
    // `concat_coercion_order`; `minimal` does not.
    let input = r#"
var a = "Hello ".concat(name, "!");
var b = "".concat(prefix, "/users/").concat(id);
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn minimal_rewrites_concat_chain_with_syntax_proven_primitive_substitutions() {
    let input = r#"
var a = "n=".concat(1, " ok=", true);
var b = "".concat(x * 2, "/").concat(typeof y);
"#;
    let expected = r#"
var a = `n=${1} ok=${true}`;
var b = `${x * 2}/${typeof y}`;
"#;
    let output = apply_minimal(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_rewrites_concat_chain_with_unproven_substitutions() {
    let input = r#"
var a = "Hello ".concat(name, "!");
var b = "".concat(prefix, "/users/").concat(id);
"#;
    let expected = r#"
var a = `Hello ${name}!`;
var b = `${prefix}/users/${id}`;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn tslib_template_helpers_restore_across_delivery_forms() {
    for (prefix, helper) in [
        ("import * as ts from 'tslib';", "ts.__makeTemplateObject"),
        ("var ts = require('tslib');", "ts.__makeTemplateObject"),
        ("import { __makeTemplateObject as h } from 'tslib';", "h"),
        ("var h = require('tslib').__makeTemplateObject;", "h"),
        ("", "require('tslib').__makeTemplateObject"),
    ] {
        let input = format!(
            "{prefix} var cache; function show(value) {{ return tag(cache || (cache = {helper}(['hello ', ''], ['hello ', ''])), value); }}"
        );
        for output in [apply(&input), render(&input)] {
            assert!(output.contains("tag`hello ${value}`"), "{output}");
        }
    }
}

#[test]
fn tslib_template_namespace_shadowing_preserves_user_calls() {
    let input = r#"
import * as ts from "tslib";
function show(ts, value) {
    return tag(ts.__makeTemplateObject(["hello ", ""], ["hello ", ""]), value);
}
"#;
    assert!(render(input).contains("ts.__makeTemplateObject("));
}

#[test]
fn tslib_template_shadowed_require_preserves_user_calls() {
    let input = r#"
function show(require, value) {
    return tag(require("tslib").__makeTemplateObject(["hello ", ""], ["hello ", ""]), value);
}
"#;
    assert!(render(input).contains(".__makeTemplateObject("));
}

#[test]
fn tslib_template_factory_restores_raw_segments() {
    let input = r#"
import * as ts from "tslib";
function data() {
    const strings = ts.__makeTemplateObject(["line\n", ""], ["line\\n", ""]);
    data = function() { return strings; };
    return strings;
}
var result = tag(data(), value);
"#;
    assert!(apply(input).contains("tag`line\\n${value}`"));
}

#[test]
fn tslib_template_dynamic_lookup_and_spread_arguments_are_preserved() {
    let input = r#"
var ts = require("tslib");
with (scope) {
    tag(ts.__makeTemplateObject(["hello"], ["hello"]));
}
tag(ts.__makeTemplateObject(...args));
"#;
    let output = apply(input);
    assert!(output.contains(".__makeTemplateObject(["), "{output}");
    assert!(
        output.contains(".__makeTemplateObject(...args)"),
        "{output}"
    );
}

#[test]
fn tslib_template_factory_is_kept_when_module_has_dynamic_scope() {
    // Restoring the tagged template consumes the helper, cache, and factory
    // bindings; the module-wide dynamic-scope skip applies even when the
    // hazard sits outside the `with` body.
    for hazard in ["eval(code);", "with (scope) { observe(); }"] {
        let input = format!(
            r#"
var ts = require("tslib");
{hazard}
function data() {{
    const strings = ts.__makeTemplateObject(["line\n", ""], ["line\\n", ""]);
    data = function() {{ return strings; }};
    return strings;
}}
var result = tag(data(), value);
"#
        );
        let output = apply(&input);
        assert!(output.contains("__makeTemplateObject"), "{output}");
        assert!(!output.contains("tag`"), "{output}");
    }
}

#[test]
fn concat_chain_recovery_ignores_dynamic_scope() {
    // `.concat` to template recovery touches no binding, so it stays active.
    let input = "with (scope) { observe(); }\nconst s = 'a'.concat(b, 'c');\n";
    let output = apply(input);
    assert!(output.contains("`a${b}c`"), "{output}");
}

/// The input text each template quasi's span starts at, in source order.
fn quasi_span_starts(input: &str) -> Vec<Option<String>> {
    struct Quasis(Vec<swc_core::common::Span>);
    impl Visit for Quasis {
        fn visit_tpl_element(&mut self, node: &TplElement) {
            self.0.push(node.span);
        }
    }
    inspect_rule_output(
        input,
        |_| UnTemplateLiteral::new(),
        |module, text| {
            let mut quasis = Quasis(Vec::new());
            module.visit_with(&mut quasis);
            quasis
                .0
                .into_iter()
                .map(|span| {
                    text.starting_at(span)
                        .map(|rest| rest.chars().take(4).collect())
                })
                .collect()
        },
    )
}

#[test]
fn template_quasis_keep_the_spans_of_their_string_literals() {
    // Plus chain: each quasi starts at its own literal. The concat call's
    // empty head quasi has no literal and keeps the whole call's span.
    assert_eq!(
        quasi_span_starts(r#"a = "x: " + U + "'"; b = "".concat(G, "-1", "!");"#),
        [
            Some(r#""x: "#.to_string()),
            Some(r#""'";"#.to_string()),
            Some(r#""".c"#.to_string()),
            Some(r#""-1""#.to_string()),
        ]
    );
}
