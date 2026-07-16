mod common;
use common::{assert_eq_normalized, render};

#[test]
fn replaces_typeof_polyfill_call_with_typeof() {
    let input = r#"
var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol" ? function(e) { return typeof e; } : function(e) {
    if (e && typeof Symbol == "function" && e.constructor === Symbol && e !== Symbol.prototype) {
        return "symbol";
    }
    return typeof e;
};
var x = _typeof(y) === "object";
"#;
    let expected = r#"
const x = typeof y === "object";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn replaces_arrow_typeof_polyfill() {
    let input = r#"
const o = typeof Symbol == "function" && typeof Symbol.iterator == "symbol" ? (e) => typeof e : (e) => {
    if (e && typeof Symbol == "function" && e.constructor === Symbol && e !== Symbol.prototype) {
        return "symbol";
    }
    return typeof e;
};
var x = o(y) === "string";
"#;
    let expected = r#"
const x = typeof y === "string";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn handles_typeof_polyfill_in_ternary_context() {
    // Usage like: (e === undefined ? "undefined" : o(e)) === "object"
    let input = r#"
const o = typeof Symbol == "function" && typeof Symbol.iterator == "symbol" ? (e) => typeof e : (e) => {
    if (e && typeof Symbol == "function" && e.constructor === Symbol && e !== Symbol.prototype) {
        return "symbol";
    }
    return typeof e;
};
var x = (y === undefined ? "undefined" : o(y)) === "object";
"#;
    // After replacing o(y) with typeof y, the ternary becomes:
    // (y === undefined ? "undefined" : typeof y) === "object"
    // which could be further simplified but that's another rule's job
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn removes_declaration_when_all_calls_replaced() {
    let input = r#"
const o = typeof Symbol == "function" && typeof Symbol.iterator == "symbol" ? (e) => typeof e : (e) => {
    if (e && typeof Symbol == "function" && e.constructor === Symbol && e !== Symbol.prototype) {
        return "symbol";
    }
    return typeof e;
};
var x = o(y);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn preserves_non_typeof_polyfill_ternary() {
    // A conditional that looks similar but isn't the typeof polyfill
    let input = r#"
const o = typeof window != "undefined" ? (e) => e.toString() : (e) => String(e);
var x = o(y);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn rewrites_imported_babel_runtime_typeof_helper() {
    // transform-runtime output imports _typeof instead of inlining it.
    let input = r#"
import _typeof from "@babel/runtime/helpers/typeof";
var x = _typeof(y) === "object";
"#;
    let expected = r#"
const x = typeof y === "object";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn rewrites_imported_swc_typeof_helper() {
    let input = r#"
import { _ as _typeof } from "@swc/helpers/_/_type_of";
var x = _typeof(y) === "object";
"#;
    let expected = r#"
const x = typeof y === "object";
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn rewrites_self_redefining_babel_typeof_helper() {
    let input = r#"
function helper(value) {
    "@babel/helpers - typeof";
    return helper = "function" == typeof Symbol && "symbol" == typeof Symbol.iterator
        ? function(value) { return typeof value; }
        : function(value) {
            return value && typeof Symbol == "function" && value.constructor === Symbol && value !== Symbol.prototype
                ? "symbol"
                : typeof value;
        }, helper(value);
}
var result = helper(input);
"#;
    let expected = r#"
const result = typeof input;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn rewrites_minified_self_redefining_typeof_helper() {
    let input = r#"
function l(e) {
    l = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
        ? function(e) { return typeof e; }
        : function(e) {
            if (e && typeof Symbol == "function" && e.constructor === Symbol && e !== Symbol.prototype) return "symbol";
            return typeof e;
        };
    return l(e);
}
var result = l(input);
"#;
    let expected = r#"
const result = typeof input;
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn preserves_aliased_self_redefining_typeof_helper() {
    let input = r#"
function helper(value) {
    helper = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
        ? function(value) { return typeof value; }
        : function(value) {
            if (value && typeof Symbol == "function" && value.constructor === Symbol && value !== Symbol.prototype) return "symbol";
            return typeof value;
        };
    return helper(value);
}
globalThis.savedTypeof = helper;
var result = helper(input);
"#;
    let output = render(input);
    assert!(output.contains("function helper(value)"), "{output}");
    assert!(output.contains("helper ="), "{output}");
    assert!(output.contains("savedTypeof = helper"), "{output}");
    assert!(output.contains("typeof input"), "{output}");
}

#[test]
fn preserves_exported_self_redefining_typeof_helper() {
    let input = r#"
export function helper(value) {
    helper = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
        ? function(value) { return typeof value; }
        : function(value) {
            if (value && typeof Symbol == "function" && value.constructor === Symbol && value !== Symbol.prototype) return "symbol";
            return typeof value;
        };
    return helper(value);
}
var result = helper(input);
"#;
    let output = render(input);
    assert!(output.contains("export function helper(value)"), "{output}");
    assert!(output.contains("helper ="), "{output}");
    assert!(output.contains("typeof input"), "{output}");
}

#[test]
fn preserves_self_redefinition_with_wrong_recursive_argument() {
    let input = r#"
function helper(value) {
    helper = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
        ? function(value) { return typeof value; }
        : function(value) {
            if (value && typeof Symbol == "function" && value.constructor === Symbol && value !== Symbol.prototype) return "symbol";
            return typeof value;
        };
    return helper(other);
}
var result = helper(input);
"#;
    let output = render(input);
    assert!(output.contains("function helper(value)"), "{output}");
    assert!(output.contains("helper(other)"), "{output}");
}

#[test]
fn preserves_typeof_conditional_with_arbitrary_fallback() {
    let input = r#"
var helper = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
    ? function(value) { return typeof value; }
    : function(value) { return "object"; };
var result = helper(input);
"#;
    let output = render(input);
    assert!(output.contains("const helper ="), "{output}");
    assert!(output.contains("helper(input)"), "{output}");
}
