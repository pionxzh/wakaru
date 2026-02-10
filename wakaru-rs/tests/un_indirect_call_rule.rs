mod common;

use common::{assert_eq_normalized, render};

#[test]
fn transforms_indirect_call_to_direct_member_call() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
import s from "react";

var countRef = (0, s.useRef)(0);
"#;
    let expected = r#"
import s from "react";
var countRef = s.useRef(0);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_multiple_indirect_calls() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
const s = require("react");
var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);
"#;
    let expected = r#"
const s = require("react");
var countRef = s.useRef(0);
var secondRef = s.useMemo(()=>{}, []);
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

