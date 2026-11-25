mod common;

use common::{assert_eq_normalized, render, render_rule};
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
fn extracts_inert_initializer_past_inert_kept_initializer() {
    // Only `index = 0` runs between the old and new position of `bag = {}`,
    // and it runs no code, so nothing can see the earlier write.
    let input = r#"
for (var index = 0, bag = {}; index < limit; index++) {
  bag[index] = index;
}
"#;
    let expected = r#"
var bag = {};
for (var index = 0; index < limit; index++) {
  bag[index] = index;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_inert_initializer_past_kept_call_in_function() {
    // The join_vars shape: `var names = Object.keys(src); var bag = {};`
    // folded into the loop header. No closure that references `bag` exists
    // while `Object.keys(src)` runs (the one in the return is created after
    // the loop), so writing `{}` first is not observable.
    let input = r#"
function collect(src) {
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {
    bag[names[index]] = src[names[index]];
  }
  return () => bag;
}
"#;
    let expected = r#"
function collect(src) {
  var bag = {};
  for (var names = Object.keys(src), index = 0; index < names.length; index++) {
    bag[names[index]] = src[names[index]];
  }
  return () => bag;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_order_among_extracted_declarators() {
    let input = r#"
function collect(src) {
  for (var setup = prepare(), names = Object.keys(src), list = [1, "a", , -2], flag = !0, label = `x`, index = 0; index < names.length; index++) {
    use(setup, list, flag, label);
  }
}
"#;
    let expected = r#"
function collect(src) {
  var setup = prepare();
  var list = [1, "a", , -2];
  var flag = !0;
  var label = `x`;
  for (var names = Object.keys(src), index = 0; index < names.length; index++) {
    use(setup, list, flag, label);
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_non_inert_or_read_initializer_behind_kept_call() {
    // Each loop keeps `bag` in the header: an initializer that is a call, an
    // object entry that reads a binding declared earlier in the header, an
    // array entry that reads a binding, a nested literal, a computed key, a
    // test that reads `bag`, and a kept initializer that mentions `bag`.
    let input = r#"
function collect(src) {
  for (var names = Object.keys(src), bag = make(), index = 0; index < names.length; index++) {}
  for (var names = Object.keys(src), bag = { size: names.length }, index = 0; index < names.length; index++) {}
  for (var names = Object.keys(src), bag = [src], index = 0; index < names.length; index++) {}
  for (var names = Object.keys(src), bag = [[]], index = 0; index < names.length; index++) {}
  for (var names = Object.keys(src), bag = { [key]: 1 }, index = 0; index < names.length; index++) {}
  for (var names = Object.keys(src), bag = {}; bag.count < names.length; bag.count++) {}
  for (var names = fill(bag), bag = {}, index = 0; index < names.length; index++) {}
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_inert_initializer_when_a_closure_or_handler_can_see_the_early_write() {
    // `Object.keys(src)` is a call, so anything that exists while it runs
    // and can reach `bag` could see `{}` written early: a closure created
    // before the loop, a hoisted function, a closure created in the header,
    // a closure from an earlier iteration of an outer loop, a `catch` reached
    // by a throw, `arguments` aliasing a parameter, or (at the top level) the
    // global object.
    let input = r#"
function before(src) {
  const peek = () => bag;
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  return peek;
}
function hoisted(src) {
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  function peek() { return bag; }
  return peek;
}
function looped(src) {
  let peek;
  while (more()) {
    for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
    peek = () => bag;
  }
  return peek;
}
function caught(src) {
  try {
    for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  } catch (err) {
    log(bag);
  }
}
function param(src, bag) {
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
}
for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn keeps_inert_initializer_behind_kept_call_when_header_creates_capturing_closure() {
    let input = r#"
function inHeader(src) {
  for (var peek = () => bag, names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  return peek;
}
"#;
    let expected = r#"
function inHeader(src) {
  var peek = () => bag;
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  return peek;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn extracts_inert_initializer_inside_outer_loop_without_capture() {
    // An outer loop only matters when a closure from an earlier iteration
    // could read the binding; with no capturing closure the move is safe.
    let input = r#"
function looped(src) {
  while (more()) {
    for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  }
}
"#;
    let expected = r#"
function looped(src) {
  while (more()) {
    var bag = {};
    for (var names = Object.keys(src), index = 0; index < names.length; index++) {}
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn pipeline_hoists_inert_initializer_from_join_vars_loop_header() {
    // producer terser@5 join_vars: the setup declarations before the loop
    // are folded into its header.
    let input = r#"
export function collect(r) {
  for (var t = Object.keys(r), e = {}, n = 0; n < t.length; n++) {
    var i = t[n];
    if (typeof r[i] === "function") e[i] = r[i];
  }
  return function () {
    return e;
  };
}
"#;
    let expected = r#"
export function collect(r) {
  const e = {};
  for (let t = Object.keys(r), n = 0; n < t.length; n++) {
    const i = t[n];
    if (typeof r[i] === "function") {
      e[i] = r[i];
    }
  }
  return function () {
    return e;
  };
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_inert_initializer_behind_kept_call_under_direct_eval() {
    let input = r#"
function collect(src) {
  for (var names = Object.keys(src), bag = {}, index = 0; index < names.length; index++) {}
  eval(code);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
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
