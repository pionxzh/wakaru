mod common;

use common::normalize;
use common::render;

#[test]
fn transforms_indirect_call_to_direct_member_call() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
import s from "react";

var countRef = (0, s.useRef)(0);
"#;
    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("var countRef = s.useRef(0);"));
}

#[test]
fn transforms_multiple_indirect_calls() {
    // Reused pattern from packages/unminify/src/transformations/__tests__/un-indirect-call.spec.ts
    let input = r#"
const s = require("react");
var countRef = (0, s.useRef)(0);
var secondRef = (0, s.useMemo)(() => {}, []);
"#;
    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("var countRef = s.useRef(0);"));
    assert!(normalized.contains("var secondRef = s.useMemo(()=>{}, []);"));
}
