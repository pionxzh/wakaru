mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::VarDeclToLetConst;

fn apply_rule(input: &str) -> String {
    render_rule(input, |_| VarDeclToLetConst)
}

#[test]
fn var_never_reassigned_becomes_const() {
    let input = r#"
var x = 1;
"#;
    let expected = r#"
const x = 1;
"#;
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn duplicate_function_and_var_binding_stays_var() {
    let input = r#"
function _dead() {}
var _dead = sideEffect();
use(_dead);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn duplicate_for_head_and_var_binding_stays_var() {
    let input = r#"
function f(xs) {
    var fns = [];
    for (var i = 0; i < xs.length; i++) {
        fns.push(function() {
            return i;
        });
    }
    var i = 10;
    return fns[0]();
}
"#;
    let expected = r#"
function f(xs) {
    const fns = [];
    for (var i = 0; i < xs.length; i++) {
        fns.push(function() {
            return i;
        });
    }
    var i = 10;
    return fns[0]();
}
"#;
    let output = apply_rule(input);
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
    let output = render_pipeline(input);
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
    let output = render_pipeline(input);
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
    let output = render_pipeline(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
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
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

// --- use-before-declaration (var hoisting) ---

#[test]
fn var_used_before_declaration_stays_var() {
    // `var` hoists, so referencing before the declaration is valid.
    // Converting to let/const would create a TDZ violation.
    let input = r#"
foo(x);
var x = 1;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn export_default_before_var_declaration_stays_var() {
    // export default references `o` before its declaration — `o` must stay var.
    // `r` is declared before its use (in `var o = r`), so it converts to const.
    let input = r#"
export default o;
var r = { a: 1 };
var o = r;
"#;
    let expected = r#"
export default o;
const r = { a: 1 };
var o = r;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_after_return_in_function_stays_var() {
    // Unreachable var still hoists; references before it rely on hoisting.
    let input = r#"
function foo(t) {
    r = t;
    return r;
    var r;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_self_reference_in_initializer_stays_var() {
    // TypeScript enum IIFE pattern: `var Foo = ((q) => { ... })(Foo || {})`
    // The self-reference `Foo || {}` relies on var hoisting (Foo is undefined).
    let input = r#"
var Sjq = ((q) => {
    q.A = "a";
    return q;
})(Sjq || {});
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_loop_head_self_reference_stays_var() {
    // `var i = i || 0` relies on hoisting (i is undefined), converting to
    // `let i = i || 0` would throw in TDZ.
    let input = r#"
for (var i = i || 0; i < 1; i++) {}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn destructuring_default_self_reference_stays_var() {
    // `var { a = a } = {}` — the default `a` references the hoisted binding.
    let input = r#"
var { a = a } = {};
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn destructuring_default_forward_reference_keeps_referenced_var() {
    // `b` in the default references a later var — `b` must stay var (hoisted).
    // `a` can safely convert since `b` remains hoisted.
    let input = r#"
var { a = b } = {};
var b = 1;
"#;
    let expected = r#"
const { a = b } = {};
var b = 1;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_for_head_self_reference_stays_var() {
    // For-head self-references must be caught even when nested inside
    // another compound statement (if/block/etc).
    let input = r#"
if (cond) {
    for (var i = i || 0; i < 1; i++) {}
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn ref_in_earlier_block_to_later_var_stays_var() {
    // Reference inside a block to a later function-scoped var.
    let input = r#"
{
    foo(x);
}
var x = 1;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn ref_in_if_body_to_later_var_stays_var() {
    let input = r#"
if (cond) foo(x);
var x = 1;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_declared_before_use_converts_normally() {
    // When the declaration comes first, conversion is safe.
    let input = r#"
var x = 1;
foo(x);
"#;
    let expected = r#"
const x = 1;
foo(x);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn known_bug_var_inside_block_used_in_sibling_block_stays_var() {
    let input = r#"
function foo(x, y) {
    if (x) {
        var a = 1;
    }
    if (y) {
        use(a);
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_used_in_for_body_before_declaration_stays_var() {
    // nv8 is used in the loop body but declared after the loop.
    // Converting to let would cause a TDZ violation.
    let input = r#"
for(var vC = 0; vC < items.length; ++vC){
    nv8 = items[vC];
    use(nv8);
}
var nv8;
"#;
    let expected = r#"
for(let vC = 0; vC < items.length; ++vC){
    nv8 = items[vC];
    use(nv8);
}
var nv8;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_used_in_for_in_body_before_declaration_stays_var() {
    let input = r#"
for(var key in obj){
    tmp = { value: obj[key] };
    use(tmp);
}
var tmp;
"#;
    let expected = r#"
for(const key in obj){
    tmp = { value: obj[key] };
    use(tmp);
}
var tmp;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn cross_declarator_for_head_reference_stays_var() {
    // `var i = j, j = 1` — i's init references j which is declared later
    // in the same for-head.  With var, j is hoisted (reads undefined).
    // With let, j is in TDZ → ReferenceError.
    let input = "for (var i = j, j = 1; i < 1; i++) {}";
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}
