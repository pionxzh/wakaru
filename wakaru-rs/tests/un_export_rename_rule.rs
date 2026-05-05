mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnExportRename;

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

#[test]
fn skips_rename_when_new_name_shadows_original_in_inner_scope() {
    // module-0 pattern: `exports.e = a` wants to rename `a → e`, but the function
    // that uses `a` also declares a local `e`. Without the shadowing check the
    // Renamer would produce `e[e]` — wrong — because both the module-level
    // renamed `a` and the local `e` print as `e` after SyntaxContext is erased.
    let input = r#"
const a = "TASK";
export const e = a;
function j() {
    let e;
    e = {};
    e[a] = true;
    return e;
}
"#;
    // Rename is skipped; the export alias is preserved.
    let expected = r#"
const a = "TASK";
export const e = a;
function j() {
    let e;
    e = {};
    e[a] = true;
    return e;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn rename_proceeds_when_inner_scope_declares_new_name_but_not_uses_old() {
    // An inner function declares `e` but never uses `a`.
    // No shadowing conflict → rename should proceed.
    let input = r#"
const a = "TASK";
export const e = a;
function unrelated() {
    let e = 42;
    return e;
}
console.log(a);
"#;
    let expected = r#"
export const e = "TASK";
function unrelated() {
    let e = 42;
    return e;
}
console.log(e);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_multi_declarator_var_decl() {
    let input = r#"
const a = 1, b = 2;
export { a as A };
"#;
    let expected = r#"
export const A = 1;
const b = 2;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_unrelated_named_export_specifiers() {
    let input = r#"
const a = 1;
const b = 2;
const Bee = 3;
export { a as A, b as Bee };
"#;
    let expected = r#"
export const A = 1;
const b = 2;
const Bee = 3;
export { b as Bee };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_other_aliases_for_same_binding() {
    let input = r#"
const a = 1;
export { a as A, a as B };
"#;
    let expected = r#"
export const A = 1;
export { A as B };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn renames_into_name_freed_by_export_plan() {
    let input = r#"
const A = (e) => new Error(e);
const I = (e) => e;
export { A as p };
export { I as A };
"#;
    let expected = r#"
export const p = (e) => new Error(e);
export const A = (e) => e;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn renames_into_name_freed_by_later_export_plan() {
    let input = r#"
const g = {
    value() {
        return 1;
    }
};
function T() {
    return 2;
}
export { T as g };
export { g as q };
"#;
    let expected = r#"
export const q = {
    value() {
        return 1;
    }
};
export function g() {
    return 2;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn resolves_export_alias_chains_to_real_binding() {
    let input = r#"
class N {}
N.propTypes = {};
const M = N;
const A = M;
export { A as Route };
"#;
    let expected = r#"
export class Route {}
Route.propTypes = {};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_object_shorthand_after_export_rename() {
    let input = r#"
const w = makeAction("push");
const S = {
    push: w
};
export { w as push };
export { S as routerActions };
"#;
    let expected = r#"
export const push = makeAction("push");
export const routerActions = {
    push
};
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
