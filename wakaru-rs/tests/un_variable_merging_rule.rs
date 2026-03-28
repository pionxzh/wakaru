mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnVariableMerging;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnVariableMerging)
}

#[test]
fn splits_var_declaration_into_individual_statements() {
    let input = r#"
var a = 1, b = true, c = "hello";
"#;
    let expected = r#"
var a = 1;
var b = true;
var c = "hello";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_let_declaration_into_individual_statements() {
    let input = r#"
let d = 1, e = 2, f = 3;
"#;
    let expected = r#"
let d = 1;
let e = 2;
let f = 3;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_const_declaration_into_individual_statements() {
    let input = r#"
const g = 1, h = 2, i = 3;
"#;
    let expected = r#"
const g = 1;
const h = 2;
const i = 3;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_single_declarator() {
    let input = r#"
var x = 1;
"#;
    let expected = r#"
var x = 1;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_export_var_declaration() {
    let input = r#"
export var a = 1, b = true, c = "hello";
"#;
    let expected = r#"
export var a = 1;
export var b = true;
export var c = "hello";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_unused_for_init_vars_before_loop() {
    let input = r#"
for (var i = 0, j = 0, k = 0; j < 10; k++) {
  console.log(k);
}
"#;
    let expected = r#"
var i = 0;
for (var j = 0, k = 0; j < 10; k++) {
  console.log(k);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_let_const_for_init() {
    // Only `var` inits are split; `let` and `const` are left alone.
    let input = r#"
for (let i = 0, j = 0, k = 0; j < 10; k++) {}
for (const i = 0, j = 0, k = 0; j < 10; k++) {}
"#;
    let expected = r#"
for (let i = 0, j = 0, k = 0; j < 10; k++) {}
for (const i = 0, j = 0, k = 0; j < 10; k++) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_extract_for_init_var_when_init_depends_on_loop_var() {
    let input = r#"
for (var n = 10, a = new Array(n), i = 0; i < n; i++) {
  a[i] = i;
}
"#;
    let expected = r#"
for (var n = 10, a = new Array(n), i = 0; i < n; i++) {
  a[i] = i;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn prunes_empty_var_decl_in_for_init_when_all_extracted() {
    let input = r#"
for (var i = 0; j < 10; k++) {}
"#;
    let expected = r#"
var i = 0;
for (; j < 10; k++) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
