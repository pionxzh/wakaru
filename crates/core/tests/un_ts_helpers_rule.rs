mod common;

use common::render;

#[test]
fn renames_awaiter_alias_and_removes_decl() {
    let input = r#"
const V = this && this.__awaiter || ((a, b, c, d) => { return new Promise(() => {}); });
export function foo() {
    return V(this, undefined, undefined, function*() {
        const x = yield fetch("url");
        return x;
    });
}
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
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
    assert!(
        output.trim().is_empty(),
        "__generator alias declaration and call should be removed: {output}"
    );
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
    insta::assert_snapshot!(output);
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
    assert!(
        output.trim().is_empty(),
        "helper declaration should be removed without touching shadowed locals: {output}"
    );
}

#[test]
fn handles_let_declaration() {
    let input = r#"
let Y = this && this.__assign || function() { return Object.assign.apply(Object, arguments); };
const x = Y({}, { a: 1 });
"#;
    let output = render(input);
    insta::assert_snapshot!(output);
}
