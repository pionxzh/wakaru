mod common;

use common::{assert_eq_normalized, render, render_rule};
use wakaru_core::rules::UnDefineProperty;

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
obj["k"] = 1;
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
(r = {})["FETCH_START"] = (e) => ({ ...e, isLoading: true });
r["FETCH_SUCCESS"] = (e, data) => ({ ...e, data });
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
    // The helper's return value (`e`) is meaningful. If the call isn't a
    // standalone statement, leave it alone — rewriting would change semantics.
    let input = r#"
function a(e, t, n) {
    if (t in e) {
        Object.defineProperty(e, t, { value: n, enumerable: true, configurable: true, writable: true });
    } else {
        e[t] = n;
    }
    return e;
}
const result = a({}, "k", 1);
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
obj["k"] = 1;
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
obj["k"] = 1;
console.log(obj);
"#;
    assert_eq_normalized(&render(input), expected);
}
