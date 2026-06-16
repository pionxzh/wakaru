mod common;

use common::assert_eq_normalized;
use wakaru_core::{decompile, DecompileOptions};

fn render_with_dce(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            dce_mode: wakaru_core::DceMode::Full,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

fn render_default(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn unused_local_undefined_initializer_is_removed() {
    let input = r#"
function load_user(app_id) {
  const response = fetch_user(app_id);
  const data = undefined;
  return response.json();
}
"#;
    let expected = r#"
function load_user(app_id) {
  const response = fetch_user(app_id);
  return response.json();
}
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn referenced_local_undefined_initializer_is_kept() {
    let input = r#"
function load_user(app_id) {
  const data = undefined;
  side(data);
  return app_id;
}
"#;
    let expected = r#"
function load_user(app_id) {
  side(undefined);
  return app_id;
}
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn top_level_undefined_initializer_is_kept() {
    let input = r#"
const data = undefined;
export const value = 1;
"#;
    let expected = r#"
const data = undefined;
export const value = 1;
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
}

#[test]
fn exported_function_is_kept() {
    let input = r#"
export function helper() { return 1; }
"#;
    let expected = r#"
export function helper() { return 1; }
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
}

#[test]
fn unreferenced_empty_temp_decl_is_removed_after_nullish_rewrite() {
    let input = r#"
function read(options) {
  var tmp;
  var keep;
  keep = options.other;
  return (tmp = options.value) !== null && tmp !== undefined ? tmp : keep;
}
export const value = read({});
"#;
    let expected = r#"
function read(options) {
  let keep;
  keep = options.other;
  return options.value ?? keep;
}
export const value = read({});
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn referenced_empty_decl_is_kept() {
    let input = r#"
function read(options) {
  var keep;
  return () => keep;
}
export const value = read({});
"#;
    let expected = r#"
function read(options) {
  let keep;
  return ()=>keep;
}
export const value = read({});
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn uninitialized_decl_is_kept_when_direct_eval_can_observe_scope() {
    let input = r#"
const o = {
  m() {
    let x;
    eval("var x;");
  }
};
"#;
    let expected = r#"
const o = {
  m() {
    let x;
    eval("var x;");
  }
};
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn uninitialized_decl_is_kept_when_parenthesized_direct_eval_can_observe_scope() {
    let input = r#"
const o = {
  m() {
    let x;
    (eval)("var x;");
  }
};
"#;
    let expected = r#"
const o = {
  m() {
    let x;
    eval("var x;");
  }
};
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn top_level_uninitialized_var_is_kept_when_global_for_in_can_observe_it() {
    let input = r#"
var enumed;
for (var key in this) {
    if (key === "__declared__var") {
        enumed = true;
    }
}
var __declared__var;
"#;
    let expected = r#"
var enumed;
for (var key in this) {
    if (key === "__declared__var") {
        enumed = true;
    }
}
var __declared__var;
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn direct_eval_does_not_keep_unrelated_function_temps() {
    let input = r#"
function read(options) {
  var tmp;
  return (tmp = options.value) !== null && tmp !== undefined ? tmp : "fallback";
}
function run(source) {
  var keep;
  eval(source);
  return keep;
}
export const value = read({}) + run("keep = 1");
"#;
    let expected = r#"
function read(options) {
  return options.value ?? "fallback";
}
function run(source) {
  var keep;
  eval(source);
  return keep;
}
export const value = read({}) + run("keep = 1");
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn static_direct_eval_keeps_only_mentioned_uninitialized_decl() {
    let input = r#"
function read() {
  var keep;
  var drop;
  eval("keep = 1");
  return keep;
}
export const value = read();
"#;
    let expected = r#"
function read() {
  var keep;
  eval("keep = 1");
  return keep;
}
export const value = read();
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn nested_direct_eval_argument_keeps_observable_uninitialized_decl() {
    let input = r#"
function read(source) {
  var keep;
  eval("0", eval(source));
  return 1;
}
export const value = read("keep = 1");
"#;
    let expected = r#"
function read(source) {
  var keep;
  eval("0", eval(source));
  return 1;
}
export const value = read("keep = 1");
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
}

#[test]
fn unknown_direct_eval_in_nested_function_keeps_outer_uninitialized_decl() {
    let input = r#"
function outer(source) {
  var keep;
  function inner() {
    eval(source);
  }
  inner();
  return keep;
}
export const value = outer("keep = 1");
"#;
    let expected = r#"
function outer(source) {
  var keep;
  function inner() {
    eval(source);
  }
  inner();
  return keep;
}
export const value = outer("keep = 1");
"#;
    assert_eq_normalized(&render_default(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
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
    assert_eq_normalized(&render_with_dce(input), expected.trim());
}

// ── Duplicate binding safety ────────────────────────────────────

#[test]
fn duplicate_binding_with_side_effect_init_is_kept() {
    let input = r#"
function _dead() {}
var _dead = sideEffect();
export const x = 2;
"#;
    let output = render_with_dce(input);
    insta::assert_snapshot!(output);
}

#[test]
fn duplicate_binding_preserves_deps_from_all_declarations() {
    let input = r#"
var a = () => b();
function a() { return 1; }
function b() { return 2; }
export const x = a();
"#;
    let output = render_with_dce(input);
    // b must survive — a's var init (`() => b()`) still references it
    insta::assert_snapshot!(output);
}
