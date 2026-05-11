mod common;

use common::{assert_eq_normalized, render};

#[test]
fn inline_esmodule_interop_unwrapped() {
    let input = r#"
const i = ((e) => {
    if (e && e.__esModule) { return e; }
    return { default: e };
})(require("./module-36.js"));
const a = i.default;
"#;
    // After full pipeline: unwrap IIFE → drop .default → UnEsm → import
    let expected = r#"
import i from "./module-36.js";
const a = i;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn inline_esmodule_interop_function_expr() {
    let input = r#"
const a = (function(e) {
    if (e && e.__esModule) { return e; }
    return { default: e };
})(require("./module-35.js"));
const b = a.default;
"#;
    let expected = r#"
import a from "./module-35.js";
const b = a;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn inline_esmodule_interop_no_default_access() {
    let input = r#"
const i = ((e) => {
    if (e && e.__esModule) { return e; }
    return { default: e };
})(require("./module-36.js"));
console.log(i);
"#;
    // IIFE unwrapped, require converted to import by UnEsm
    let expected = r#"
import i from "./module-36.js";
console.log(i);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn non_interop_iife_unchanged() {
    let input = r#"
const x = ((e) => {
    const a = e + 1;
    const b = a * 2;
    return b;
})(42);
console.log(x);
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
