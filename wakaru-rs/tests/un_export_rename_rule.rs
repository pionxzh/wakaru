mod common;

use wakaru_rs::rules::UnExportRename;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnExportRename)
}

#[test]
fn export_const_inlines_var_declaration() {
    let input = r#"
const a = 1;
export const App = a;
"#;
    let expected = r#"
export const App = 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_const_inlines_function_declaration() {
    let input = r#"
function a() {}
export const App = a;
"#;
    let expected = r#"
export function App() {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_const_inlines_class_declaration() {
    let input = r#"
class o {}
export const App = o;
"#;
    let expected = r#"
export class App {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_specifier_inlines_var_declaration() {
    let input = r#"
const o = { a: 1 };
export { o as Game };
"#;
    let expected = r#"
export const Game = { a: 1 };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_specifier_inlines_function_declaration() {
    let input = r#"
function o() { return 1; }
export { o as compute };
"#;
    let expected = r#"
export function compute() { return 1; }
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn skips_when_new_name_already_declared() {
    // 'App' is already declared — skip the rename
    let input = r#"
const o = 1;
const App = 2;
export { o as App };
"#;
    let expected = r#"
const o = 1;
const App = 2;
export { o as App };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn renames_all_usages() {
    let input = r#"
const a = 1;
export const Counter = a;
console.log(a);
"#;
    let expected = r#"
export const Counter = 1;
console.log(Counter);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_rename_does_not_touch_shadowed_local() {
    let input = r#"
const l = 1;
export const StrictMode = l;
function createElement() {
  let l = 2;
  return l;
}
console.log(l);
"#;
    let expected = r#"
export const StrictMode = 1;
function createElement() {
  let l = 2;
  return l;
}
console.log(StrictMode);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

