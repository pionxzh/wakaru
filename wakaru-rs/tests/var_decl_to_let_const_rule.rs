mod common;

use common::{assert_eq_normalized, render};

#[test]
fn var_never_reassigned_becomes_const() {
    let input = r#"
var x = 1;
"#;
    let expected = r#"
const x = 1;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_reassigned_becomes_let() {
    let input = r#"
var x = 1;
x = 2;
"#;
    let expected = r#"
let x = 1;
x = 2;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_updated_becomes_let() {
    let input = r#"
var i = 0;
i++;
"#;
    let expected = r#"
let i = 0;
i++;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_without_init_becomes_let() {
    let input = r#"
var x;
x = 10;
"#;
    let expected = r#"
let x;
x = 10;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_inside_function_scope() {
    let input = r#"
function foo() {
    var a = 1;
    var b = 2;
    b = 3;
    return a + b;
}
"#;
    let expected = r#"
function foo() {
    const a = 1;
    let b = 2;
    b = 3;
    return a + b;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_assigned_in_nested_closure_becomes_let() {
    let input = r#"
var counter = 0;
function inc() {
    counter++;
}
"#;
    let expected = r#"
let counter = 0;
function inc() {
    counter++;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_same_name_different_scope_no_false_let() {
    // Outer `r` is never reassigned in its own scope.
    // Inner function has a shadowing `r` that IS reassigned.
    // Scope-aware tracking must not conflate the two: outer → const, inner → let.
    let input = r#"
var r = outerModule;
function factory() {
    var r = innerModule;
    r = otherModule;
    return r;
}
"#;
    let expected = r#"
const r = outerModule;
function factory() {
    let r = innerModule;
    r = otherModule;
    return r;
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

// --- multi-variable declarations ---

#[test]
fn multi_var_none_reassigned_each_becomes_const() {
    // UnVariableMerging runs before VarDeclToLetConst and splits multi-var declarations.
    // Each individual declarator is then analyzed and upgraded independently.
    let input = r#"
var x = 1, y = 2;
"#;
    let expected = r#"
const x = 1;
const y = 2;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_var_all_reassigned_each_becomes_let() {
    // After UnVariableMerging splits the declaration, each var is converted to let
    // because both are reassigned
    let input = r#"
var x = 1, y = 2;
x = 3;
y = 4;
"#;
    let expected = r#"
let x = 1;
let y = 2;
x = 3;
y = 4;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_var_mixed_splits_into_separate_decls() {
    // When declarators need different keywords, split into separate declarations
    let input = r#"
var x = 1, y = 2;
y = 4;
"#;
    let expected = r#"
const x = 1;
let y = 2;
y = 4;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

// --- for-loop heads ---

#[test]
fn for_loop_var_becomes_let() {
    // Loop counter is reassigned on every iteration — must be `let`
    let input = r#"
for (var i = 0; i < 10; i++) { foo(i); }
"#;
    let expected = r#"
for (let i = 0; i < 10; i++) { foo(i); }
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_in_var_becomes_const() {
    // The iteration variable in `for...in` is assigned once per iteration and
    // never reassigned inside the body — use `const`
    let input = r#"
for (var key in obj) { foo(key); }
"#;
    let expected = r#"
for (const key in obj) { foo(key); }
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

// --- block-scope escape analysis ---

#[test]
fn var_inside_block_referenced_outside_stays_var() {
    // `var` is function-scoped; if it's referenced outside the block it was
    // declared in, converting to `let`/`const` would change semantics
    let input = r#"
if (true) {
    var a = 1;
}
foo(a);
"#;
    let output = render(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_inside_block_not_used_outside_becomes_const() {
    // When the `var` never escapes its block, it can safely become block-scoped
    let input = r#"
if (true) {
    var a = 1;
    foo(a);
}
"#;
    let expected = r#"
if (true) {
    const a = 1;
    foo(a);
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
