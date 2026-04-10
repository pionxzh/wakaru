use std::fs;

use wakaru_rs::{unpack, DecompileOptions};

mod common;
use common::{assert_eq_normalized, render};

#[test]
fn export_named_renames_class() {
    let input = r#"
class p {
    render() { return 42; }
}
p.propTypes = {};
export const BrowserRouter = p;
"#;
    let output = render(input);
    assert!(output.contains("class BrowserRouter"), "class should be renamed: {output}");
    assert!(!output.contains("class p "), "old name should be gone: {output}");
    assert!(!output.contains("export const BrowserRouter = "), "export const alias should be removed: {output}");
}

#[test]
fn export_named_renames_through_alias() {
    // var h = p; export { h as BrowserRouter } — should resolve alias and rename p
    let input = r#"
class p {
    render() { return 42; }
}
var h = p;
export const BrowserRouter = h;
"#;
    let output = render(input);
    assert!(output.contains("BrowserRouter"), "should contain BrowserRouter: {output}");
    // h should be inlined away, class should be renamed
    assert!(!output.contains("var h"), "alias should be removed: {output}");
}

#[test]
fn export_named_multiple_classes() {
    let input = r#"
class p {
    render() { return 1; }
}
p.propTypes = {};
class y {
    render() { return 2; }
}
y.propTypes = {};
export const Foo = p;
export const Bar = y;
"#;
    let output = render(input);
    assert!(output.contains("class Foo"), "p should become Foo: {output}");
    assert!(output.contains("class Bar"), "y should become Bar: {output}");
}

#[test]
fn webpack4_module_24_renames_classes() {
    let source_path = "../testcases/webpack4/dist/index.js";
    let Ok(source) = fs::read_to_string(source_path) else {
        eprintln!("skipping: webpack4 testcase not found");
        return;
    };

    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");

    let (_, code) = pairs
        .iter()
        .find(|(name, _)| name == "module-24.js")
        .expect("module-24 should exist");

    assert!(
        !code.contains("export const BrowserRouter = p"),
        "BrowserRouter should be inlined as export class, not export const = p"
    );
    assert!(
        code.contains("BrowserRouter"),
        "BrowserRouter export should exist"
    );
}

#[test]
fn alias_used_beyond_export_is_also_renamed() {
    // P1 regression: var h = p; console.log(h); export { h as BrowserRouter }
    // Removing `var h = p` without renaming remaining `h` refs leaves dangling references
    let input = r#"
class p {
    render() { return 42; }
}
var h = p;
console.log(h);
export { h as BrowserRouter };
"#;
    let output = render(input);
    assert!(output.contains("BrowserRouter"), "should contain BrowserRouter: {output}");
    assert!(!output.contains("console.log(h)"), "h should be renamed, not left dangling: {output}");
    assert!(output.contains("console.log(BrowserRouter)"), "h should become BrowserRouter: {output}");
}
