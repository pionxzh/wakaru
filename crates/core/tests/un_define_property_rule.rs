mod common;

use common::{assert_eq_normalized, render, render_pipeline_until_with_level, render_rule};
use wakaru_core::rules::{RewriteLevel, UnDefineProperty};

// Body of the _defineProperty helper:
//   function X(e, t, n) {
//       if (t in e) {
//           Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
//       } else {
//           e[t] = n;
//       }
//       return e;
//   }

#[test]
fn detects_and_rewrites_helper_call() {
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const obj = {};
a(obj, "k", 1);
console.log(obj);
"#;
    let expected = r#"
const obj = {};
obj.k = 1;
console.log(obj);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn rewrites_assignment_wrap_form() {
    // Module-36 shape: `a(r = {}, KEY, VALUE)` used to seed an object.
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
let r;
a(r = {}, "FETCH_START", (e) => ({ ...e, isLoading: true }));
a(r, "FETCH_SUCCESS", (e, t) => ({ ...e, data: t }));
console.log(r);
"#;
    let expected = r#"
let r;
(r = {}).FETCH_START = (e) => ({ ...e, isLoading: true });
r.FETCH_SUCCESS = (e, data) => ({ ...e, data });
console.log(r);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn detects_compact_babel_helper_after_normalization() {
    // Raw webpack module-36 shape: Babel's helper starts as a ternary return
    // wrapped in a comma sequence, so UnConditionals must normalize it first.
    let input = r#"
function a(e, t, n) {
    return t in e ? Object.defineProperty(e, t, {
        value: n,
        enumerable: !0,
        configurable: !0,
        writable: !0
    }) : e[t] = n, e;
}
let r;
a(r = {}, FETCH_DATA + START, (e) => ({ ...e, isLoading: true }));
a(r, FETCH_DATA + SUCCESS, (e, t) => ({ ...e, data: t }));
console.log(r);
"#;
    let expected = r#"
let r;
(r = {})[FETCH_DATA + START] = (e) => ({ ...e, isLoading: true });
r[FETCH_DATA + SUCCESS] = (e, data) => ({ ...e, data });
console.log(r);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn removes_helper_after_all_call_sites_rewritten() {
    // When helper's only references are calls we rewrote, drop the decl.
    let input = r#"
function _defineProperty(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const obj = {};
_defineProperty(obj, "k", 1);
console.log(obj);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn descriptor_missing_enumerable_configurable_not_detected() {
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const obj = {};
a(obj, "k", 1);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn unrelated_three_param_function_not_detected() {
    // Function with 3 params but wrong body — must not be matched as helper.
    let input = r#"
function add3(a, b, c) {
    return a + b + c;
}
const x = add3(1, 2, 3);
console.log(x);
"#;
    let expected = r#"
function add3(a, b, c) {
    return a + b + c;
}
const x = add3(1, 2, 3);
console.log(x);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn non_statement_helper_call_is_left_intact() {
    // The helper's return value (`e`) is meaningful for an existing target.
    // Only the producer-proven fresh-object form has an expression rewrite.
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const target = {};
const result = a(target, "k", 1);
console.log(result);
"#;
    // `a` must still be called somehow; helper not removed (still referenced).
    let output = render(input);
    insta::assert_snapshot!(output);
}

#[test]
fn standalone_rule_detects_to_property_key_normalized_helper() {
    // Regression: the standalone `VisitMut` path now classifies helpers through
    // the shared `LocalHelperContext` instead of a private copy of the matcher.
    // The shared matcher tolerates modern Babel/SWC key normalization
    // (`(t = _toPropertyKey(t)) in e`); the old rule-local copy only accepted a
    // bare `t in e` and missed this form when the rule ran standalone.
    let input = r#"
function _defineProperty(e, t, n) {
    if ((t = _toPropertyKey(t)) in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const obj = {};
_defineProperty(obj, "k", 1);
console.log(obj);
"#;
    let expected = r#"
const obj = {};
obj.k = 1;
console.log(obj);
"#;
    assert_eq_normalized(&render_rule(input, |_| UnDefineProperty), expected.trim());
}

#[test]
fn rewrites_swc_external_define_property_import() {
    let input = r#"
import { _ as _define_property } from "@swc/helpers/_/_define_property";
const obj = {};
_define_property(obj, "k", 1);
console.log(obj);
"#;
    let expected = r#"
const obj = {};
obj.k = 1;
console.log(obj);
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn recovers_computed_property_from_fresh_object_call() {
    let input = r#"
function helper(target, key, value) {
    if (key in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
const result = helper({}, key, makeValue());
"#;
    let expected = r#"
const result = { [key]: makeValue() };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn recovers_fresh_object_call_in_nested_expression() {
    let input = r#"
function helper(target, key, value) {
    if (key in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
consume({ metadata: helper({}, key, value) });
"#;
    let expected = r#"
consume({ metadata: { [key]: value } });
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn minimal_preserves_fresh_object_call_evaluation_order() {
    let input = r#"
function helper(target, key, value) {
    if (key in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
const result = helper({}, keyObject, makeValue());
"#;
    let output = render_pipeline_until_with_level(input, "UnDefineProperty", RewriteLevel::Minimal);
    assert!(
        output.contains("helper({}, keyObject, makeValue())"),
        "{output}"
    );
}

#[test]
fn expression_calls_with_nonempty_targets_are_preserved() {
    let input = r#"
function helper(target, key, value) {
    if (key in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
const first = helper({ existing: 1 }, key, value);
const second = helper({ ...base }, key, value);
"#;
    let output = render(input);
    assert!(output.contains("helper({\n    existing: 1"), "{output}");
    assert!(output.contains("helper({\n    ...base"), "{output}");
}

#[test]
fn remaining_expression_call_keeps_helper_declaration() {
    let input = r#"
function helper(target, key, value) {
    if (key in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
const fresh = helper({}, key, value);
const existing = helper(target, key, value);
"#;
    let output = render(input);
    assert!(
        output.contains("function helper(target, key, value)"),
        "{output}"
    );
    assert!(
        output.contains("const fresh = {\n    [key]: value"),
        "{output}"
    );
    assert!(output.contains("helper(target, key, value)"), "{output}");
}

#[test]
fn fresh_object_recovery_removes_helper_dependencies() {
    let input = r#"
function primitive(value) {
    return value;
}
function propertyKey(value) {
    const key = primitive(value);
    return typeof key === "symbol" ? key : String(key);
}
function helper(target, key, value) {
    if ((key = propertyKey(key)) in target) {
        Object.defineProperty(target, key, { value: value, enumerable: true, configurable: true, writable: true });
    } else {
        target[key] = value;
    }
    return target;
}
const result = helper({}, key, value);
"#;
    let expected = r#"
const result = { [key]: value };
"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn side_effect_imports_survive_helper_removal() {
    // Removing the helper's own imports must not sweep up imports that never
    // had a specifier to begin with.
    let input = r#"
import "./side.js";
import "./other.css";
function a(e, t, n) {
    return (t = i(t)) in e ? Object.defineProperty(e, t, { value: n, enumerable: !0, configurable: !0, writable: !0 }) : e[t] = n, e;
}
function i(e) {
    var t = function(e, t) {
        if ("object" != typeof e || !e) return e;
        var n = e[Symbol.toPrimitive];
        if (void 0 !== n) {
            var o = n.call(e, t || "default");
            if ("object" != typeof o) return o;
            throw new TypeError("@@toPrimitive must return a primitive value.");
        }
        return ("string" === t ? String : Number)(e);
    }(e, "string");
    return "symbol" == typeof t ? t : t + "";
}
var o = {};
a(o, "x", 1);
export { o };
"#;
    let output = render(input);
    assert!(output.contains(r#"import "./side.js";"#), "{output}");
    assert!(output.contains(r#"import "./other.css";"#), "{output}");
    assert!(!output.contains("function a("), "{output}");
}

#[test]
fn helper_dependency_stays_while_a_surviving_helper_still_calls_it() {
    // `a` (_defineProperty) goes away, `i` (toPropertyKey) stays because `o`
    // (_defineProperties) still calls it, so `r` (_typeof), which only `i`
    // calls, must stay too.
    let input = r#"
function r(e) {
    return (r = "function" == typeof Symbol && "symbol" == typeof Symbol.iterator ? function(e) {
        return typeof e;
    } : function(e) {
        return e && "function" == typeof Symbol && e.constructor === Symbol && e !== Symbol.prototype ? "symbol" : typeof e;
    })(e);
}
function o(e, t) {
    for (var n = 0; n < t.length; n++) {
        var r = t[n];
        r.enumerable = r.enumerable || !1;
        r.configurable = !0;
        "value" in r && (r.writable = !0);
        Object.defineProperty(e, i(r.key), r);
    }
}
function a(e, t, n) {
    return (t = i(t)) in e ? Object.defineProperty(e, t, { value: n, enumerable: !0, configurable: !0, writable: !0 }) : e[t] = n, e;
}
function i(e) {
    var t = function(e, t) {
        if ("object" != r(e) || !e) return e;
        var n = e[Symbol.toPrimitive];
        if (void 0 !== n) {
            var o = n.call(e, t || "default");
            if ("object" != r(o)) return o;
            throw new TypeError("@@toPrimitive must return a primitive value.");
        }
        return ("string" === t ? String : Number)(e);
    }(e, "string");
    return "symbol" == r(t) ? t : t + "";
}
var c = {};
a(c, "x", 1);
o(c, [{ key: "y", value: 2 }]);
export { c };
"#;
    let output = render(input);
    assert!(!output.contains("function a("), "{output}");
    assert!(output.contains("function o("), "{output}");
    assert!(output.contains("function i("), "{output}");
    let calls_typeof_helper = output.contains(" r(") || output.contains("(r(");
    assert!(
        !calls_typeof_helper || output.contains("function r("),
        "typeof helper is called but not declared:\n{output}"
    );
}

#[test]
fn normalizes_literal_keys_it_synthesizes() {
    // UnDefineProperty runs after UnBracketNotation, so it normalizes the
    // keys it builds itself.
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const obj = {};
a(obj, "default", 1);
a(obj, "1", 2);
a(obj, "a-b", 3);
a(obj, k, 4);
use(a({}, "name", 5), a({}, "x-y", 6));
"#;
    let expected = r#"
const obj = {};
obj.default = 1;
obj[1] = 2;
obj["a-b"] = 3;
obj[k] = 4;
use({ name: 5 }, { ["x-y"]: 6 });
"#;
    assert_eq_normalized(&render(input), expected.trim());
}
