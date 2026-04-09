mod common;

use common::{assert_eq_normalized, render};

#[test]
fn renames_awaiter_alias_and_removes_decl() {
    let input = r#"
const V = this && this.__awaiter || ((a, b, c, d) => { return new Promise(() => {}); });
function foo() {
    return V(this, undefined, undefined, function*() {
        const x = yield fetch("url");
        return x;
    });
}
"#;
    let output = render(input);
    // The helper decl should be removed
    assert!(!output.contains("this.__awaiter"), "helper decl should be removed");
    // The function should become async
    assert!(output.contains("async"), "function should be async: {output}");
}

#[test]
fn renames_generator_alias_and_removes_decl() {
    let input = r#"
const Z = this && this.__generator || ((a, b) => { });
function foo() {
    return Z(this, function(state) {
        switch(state.label) {
            case 0:
                return [2, 42];
        }
    });
}
"#;
    let output = render(input);
    // The helper decl should be removed
    assert!(!output.contains("this.__generator"), "helper decl should be removed");
}

#[test]
fn does_not_touch_non_helper_patterns() {
    let input = r#"
const V = someOtherThing || fallback;
function foo() {
    return V(1, 2, 3);
}
"#;
    let output = render(input);
    assert!(output.contains("someOtherThing"), "non-helper should be preserved");
}

#[test]
fn does_not_rename_shadowed_locals() {
    // Regression: HelperRenamer was renaming ALL identifiers with the same name,
    // including inner-scope locals that shadow the helper alias.
    let input = r#"
const Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
function foo(Y) {
    let Y = "";
    return Y;
}
"#;
    let output = render(input);
    // Inner `Y` params/vars should NOT become `__assign`
    assert!(!output.contains("function foo(__assign)"), "param should not be renamed: {output}");
    assert!(!output.contains("let __assign"), "inner let should not be renamed: {output}");
}

#[test]
fn handles_let_declaration() {
    let input = r#"
let Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
const x = Y({}, { a: 1 });
"#;
    let output = render(input);
    // Helper decl should be removed
    assert!(!output.contains("this.__assign"), "helper decl should be removed");
    // The call should use __assign
    assert!(output.contains("__assign"), "call should use canonical name: {output}");
}
