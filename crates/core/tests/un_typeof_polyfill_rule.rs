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
    assert!(
        output.contains("typeof y"),
        "should replace o(y) with typeof y"
    );
    assert!(
        !output.contains("Symbol"),
        "should remove polyfill declaration"
    );
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
    assert!(
        !output.contains("Symbol"),
        "polyfill declaration should be removed"
    );
}

#[test]
fn preserves_non_typeof_polyfill_ternary() {
    // A conditional that looks similar but isn't the typeof polyfill
    let input = r#"
const o = typeof window != "undefined" ? (e) => e.toString() : (e) => String(e);
var x = o(y);
"#;
    let output = render(input);
    assert!(
        output.contains("o(y)") || output.contains("o("),
        "should not transform unrelated conditional"
    );
}
