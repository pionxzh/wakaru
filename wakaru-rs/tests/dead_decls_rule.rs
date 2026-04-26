mod common;

use common::{assert_eq_normalized, render};

// ── Unreferenced function declarations ──────────────────────────

#[test]
fn unreferenced_function_is_removed() {
    let input = r#"
function _helper() { return 1; }
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn referenced_function_is_kept() {
    let input = r#"
function helper() { return 1; }
export const x = helper();
"#;
    let expected = r#"
function helper() { return 1; }
export const x = helper();
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn self_recursive_unreferenced_function_is_removed() {
    let input = r#"
function _helper(n) { return n <= 0 ? 1 : _helper(n - 1); }
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn exported_function_is_kept() {
    let input = r#"
export function helper() { return 1; }
"#;
    let expected = r#"
export function helper() { return 1; }
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Unreferenced variable declarations (function/arrow init) ────

#[test]
fn unreferenced_const_fn_expr_is_removed() {
    let input = r#"
const _helper = function() { return 1; };
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn unreferenced_const_arrow_is_removed() {
    let input = r#"
const _helper = () => 1;
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn referenced_const_arrow_is_kept() {
    let input = r#"
const helper = () => 1;
export const x = helper();
"#;
    let expected = r#"
const helper = ()=>1;
export const x = helper();
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Non-function inits are NOT candidates ───────────────────────

#[test]
fn unreferenced_const_literal_is_kept() {
    let input = r#"
const _UNUSED = "hello";
export const x = 2;
"#;
    let expected = r#"
const _UNUSED = "hello";
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn unreferenced_const_with_call_init_is_kept() {
    let input = r#"
const _result = sideEffect();
export const x = 2;
"#;
    let expected = r#"
const _result = sideEffect();
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Multi-declarator handling ───────────────────────────────────

#[test]
fn mixed_declarators_keep_non_function_ones() {
    let input = r#"
const _dead = () => 1, alive = () => 2;
export const x = alive();
"#;
    let expected = r#"
const alive = ()=>2;
export const x = alive();
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Iterative removal (chain of dead references) ────────────────

#[test]
fn chained_dead_functions_are_removed() {
    let input = r#"
function _a() { return 1; }
function _b() { return _a(); }
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Exported variable declarations are kept ─────────────────────

#[test]
fn exported_const_is_kept() {
    let input = r#"
export const helper = () => 1;
"#;
    let expected = r#"
export const helper = ()=>1;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

// ── Mutual recursion ────────────────────────────────────────────

#[test]
fn mutually_recursive_unreferenced_functions_are_removed() {
    let input = r#"
function _a(n) { return n <= 0 ? 1 : _b(n - 1); }
function _b(n) { return _a(n); }
export const x = 2;
"#;
    let expected = r#"
export const x = 2;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}
