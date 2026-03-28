mod common;

use common::{assert_eq_normalized, render_pipeline};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn transforms_indirect_call_to_direct_member_call() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
import s from "react";

var countRef = (0, s.useRef)(0);
"#;
    // VarDeclToLetConst converts var to const since countRef is never reassigned.
    let expected = r#"
import s from "react";
const countRef = s.useRef(0);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_multiple_indirect_calls() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    // UnEsm converts `const s = require("react")` → `import s from "react"`
    let input = r#"
const s = require("react");
var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);
"#;
    // VarDeclToLetConst converts var to const since these vars are never reassigned.
    let expected = r#"
import s from "react";
const countRef = s.useRef(0);
const secondRef = s.useMemo(()=>{}, []);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_object_wrap_indirect_call() {
    // Object(fn.method)(args) → fn.method(args)
    // webpack bundles use Object() to avoid `this` binding on member expressions
    let input = r#"
Object(r.h)(e, "msg");
Object(r.validate)(x);
"#;
    let expected = r#"
r.h(e, "msg");
r.validate(x);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}


