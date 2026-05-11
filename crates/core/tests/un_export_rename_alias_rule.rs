use std::fs;

use wakaru_core::{unpack, DecompileOptions};

mod common;
use common::render;

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
    insta::assert_snapshot!(output);
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
    insta::assert_snapshot!(output);
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
    insta::assert_snapshot!(output);
}

#[test]
fn webpack4_module_24_renames_classes() {
    let source_path = "../../testcases/webpack4/dist/index.js";
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

    insta::assert_snapshot!(code);
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
    insta::assert_snapshot!(output);
}
