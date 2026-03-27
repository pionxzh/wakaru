mod common;

use common::{assert_eq_normalized, render};

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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_unused_for_init_vars_before_loop() {
    // `i` is not used in test (j < 10) or update (k++), so it gets extracted.
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
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn prunes_empty_var_decl_in_for_init_when_all_extracted() {
    // All declarators are extracted, so the for init becomes None.
    let input = r#"
for (var i = 0; j < 10; k++) {}
"#;
    let expected = r#"
var i = 0;
for (; j < 10; k++) {}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
