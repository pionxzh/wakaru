mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnVariableMerging;

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
fn preserves_initializer_evaluation_order_with_kept_suffix() {
    let input = r#"
for (var a = A(), n = N(), b = B(); n < limit; n++) {}
"#;
    let expected = r#"
var a = A();
for (var n = N(), b = B(); n < limit; n++) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_uninitialized_declarator_past_kept_initializer() {
    // A declarator with no initializer evaluates nothing, so pulling it out of
    // the header cannot reorder effects even when an earlier declarator stays.
    // This is the swc/babel iterator-protocol shape UnForOf matches on.
    let input = r#"
for (var iterator = items[Symbol.iterator](), step; !(done = (step = iterator.next()).done); done = true) {
  use(step.value);
}
"#;
    let expected = r#"
var step;
for (var iterator = items[Symbol.iterator](); !(done = (step = iterator.next()).done); done = true) {
  use(step.value);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_later_initializer_behind_kept_initializer() {
    // `second = makeSecond()` must not move: it would run before the kept
    // `index = start()` and reorder their side effects.
    let input = r#"
for (var index = start(), second = makeSecond(), gap; index < limit; index++) {
  use(second, gap);
}
"#;
    let expected = r#"
var gap;
for (var index = start(), second = makeSecond(); index < limit; index++) {
  use(second, gap);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_only_setup_from_terser_merged_loop_init() {
    // Terser 5 currently merges the preceding setup declaration into this
    // for-init shape. Keeping the suffix preserves prepare → start → first.
    let input = r#"
for (var setup = prepare(), index = start(), current = first(); index < limit; index++) {
  use(current);
}
"#;
    let expected = r#"
var setup = prepare();
for (var index = start(), current = first(); index < limit; index++) {
  use(current);
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
