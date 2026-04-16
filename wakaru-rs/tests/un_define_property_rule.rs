mod common;

use common::{assert_eq_normalized, render};

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
r["FETCH_SUCCESS"] = (e, t) => ({ ...e, data: t });
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
    assert!(
        !output.contains("function _defineProperty"),
        "helper should be removed when all call sites are rewritten; got:\n{output}"
    );
    assert!(output.contains(r#"obj["k"] = 1"#), "got:\n{output}");
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
    assert!(
        output.contains("function a(e, t, n)"),
        "helper must be kept when a call isn't a rewritable statement; got:\n{output}"
    );
    assert!(
        output.contains("a({}, \"k\", 1)") || output.contains("a({}, 'k', 1)"),
        "original call should be preserved; got:\n{output}"
    );
}
