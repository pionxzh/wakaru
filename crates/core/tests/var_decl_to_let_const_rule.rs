mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::{rules::VarDeclToLetConst, RewriteLevel};

fn apply_rule(input: &str) -> String {
    render_rule(input, |_| VarDeclToLetConst::new())
}

fn apply_rule_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| VarDeclToLetConst::new_with_level(level))
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
fn var_self_referenced_only_inside_arrow_initializer_becomes_const() {
    let input = r#"
var r = (e, t) => {
    if (r.dependsOnOwnProps) {
        return r.mapToProps(e, t);
    }
    return r.mapToProps(e);
};
r.dependsOnOwnProps = true;
"#;
    let expected = r#"
const r = (e, t) => {
    if (r.dependsOnOwnProps) {
        return r.mapToProps(e, t);
    }
    return r.mapToProps(e);
};
r.dependsOnOwnProps = true;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_referenced_by_nested_callback_in_initializer_stays_var() {
    let input = r#"
function eventChannel(subscribe) {
    var unsubscribe = subscribe(() => {
        if (unsubscribe) {
            unsubscribe();
        }
    });
    return unsubscribe;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_captured_by_earlier_closure_used_during_initializer_stays_var() {
    let input = r#"
function eventChannel(subscribe) {
    const close = () => {
        if (unsubscribe) {
            unsubscribe();
        }
    };
    var unsubscribe = subscribe(() => close());
    return unsubscribe;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_read_by_hoisted_function_called_before_initializer_stays_var() {
    let input = r#"
function readBeforeInit() {
    var value = read();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;
    let expected = r#"
function readBeforeInit() {
    const value = read();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_read_by_hoisted_function_called_before_later_decl_stays_var() {
    let input = r#"
function readBeforeInit() {
    read();
    var limit = 1;
    function read() { return limit; }
    return limit;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_read_by_parenthesized_hoisted_function_called_before_initializer_stays_var() {
    let input = r#"
function readBeforeInit() {
    var value = (read)();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;
    let expected = r#"
function readBeforeInit() {
    const value = read();
    var limit = 1;
    function read() { return limit; }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_read_by_hoisted_constructor_called_before_initializer_stays_var() {
    let input = r#"
function readBeforeInit() {
    var value = new Read();
    var limit = 1;
    function Read() { this.value = limit; }
    return value;
}
"#;
    let expected = r#"
function readBeforeInit() {
    const value = new Read();
    var limit = 1;
    function Read() { this.value = limit; }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_read_by_parenthesized_hoisted_constructor_called_before_initializer_stays_var() {
    let input = r#"
function readBeforeInit() {
    var value = new (Read)();
    var limit = 1;
    function Read() { this.value = limit; }
    return value;
}
"#;
    let expected = r#"
function readBeforeInit() {
    const value = new Read();
    var limit = 1;
    function Read() { this.value = limit; }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn earlier_hoisted_function_call_allows_already_initialized_var_to_be_const() {
    let input = r#"
function readAfterInit() {
    var limit = 1;
    var value = read();
    function read() { return limit; }
    return value;
}
"#;
    let expected = r#"
function readAfterInit() {
    const limit = 1;
    const value = read();
    function read() { return limit; }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn earlier_function_initializer_reference_to_later_var_stays_var() {
    let input = r#"
var read = function() {
    return value;
};
var value = 1;
"#;
    let expected = r#"
const read = function() {
    return value;
};
var value = 1;
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

#[test]
fn for_head_var_referenced_after_loop_stays_var() {
    let input = r#"
function walk(node) {
    for (var parent = node.parent; parent !== null;) {
        node = parent;
        parent = parent.parent;
    }
    if (node.kind === "root") {
        parent = node.state;
        return parent;
    }
    return null;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_in_head_var_referenced_after_loop_stays_var() {
    let input = r#"
function lastKey(obj) {
    for (var key in obj) {
        visit(key);
    }
    return key;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_of_head_var_referenced_after_loop_stays_var() {
    let input = r#"
function lastValue(values) {
    for (var value of values) {
        visit(value);
    }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_used_by_lexical_decl_before_for_head_stays_var() {
    let input = r#"
const re = new RegExp(`[${items.map((item) => item.name).join("")}]`);
for (var items = [{ name: "a" }], i = 0; i < items.length; i++) {
    use(items[i]);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_used_by_lexical_decl_initializer_before_declaration_stays_var() {
    let input = r#"
const y = x + 1;
var x = 1;
"#;
    let expected = r#"
const y = x + 1;
var x = 1;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_named_let_stays_var() {
    let input = r#"
var let = 1;
var object = { let };
"#;
    let expected = r#"
var let = 1;
const object = { let };
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_var_observed_through_global_this_stays_var() {
    let input = r#"
var v = 1;
assert.sameValue(globalThis.v, 1);
"#;
    let expected = r#"
var v = 1;
assert.sameValue(globalThis.v, 1);
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_var_observed_through_top_level_this_stays_var() {
    let input = r#"
var v = 1;
delete this.v;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn module_var_observed_by_top_level_global_for_in_stays_var() {
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
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_var_observed_in_global_with_stays_var() {
    let input = r#"
var v = 1;
with (globalThis) {
    assert.sameValue(v, 1);
}
"#;
    let expected = r#"
var v = 1;
with (globalThis) {
    assert.sameValue(v, 1);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_inside_with_body_stays_var() {
    let input = r#"
with (obj) {
    var value = "set";
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn module_var_referenced_from_with_body_stays_var() {
    let input = r#"
var b;
with (obj) {
    b = true;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn exported_module_var_stays_var_in_minimal() {
    let input = r#"
import { x as y } from './self.js';
assert.sameValue(y, undefined);
export var x = 23;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(
        &output,
        r#"
import { x as y } from './self.js';
assert.sameValue(y, undefined);
export const x = 23;
"#,
    );

    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn module_var_exported_by_named_export_stays_var_in_minimal() {
    let input = r#"
import { x as y } from './self.js';
assert.sameValue(y, undefined);
var x = 23;
export { x };
"#;
    let output = apply_rule(input);
    assert_eq_normalized(
        &output,
        r#"
import { x as y } from './self.js';
assert.sameValue(y, undefined);
const x = 23;
export { x };
"#,
    );

    let output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_of_var_named_let_stays_var() {
    let input = r#"
var iterCount = 0;
for (var let of [23]) {
    iterCount += let;
}
"#;
    let expected = r#"
let iterCount = 0;
for (var let of [23]) {
    iterCount += let;
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

#[test]
fn for_of_assignment_destructuring_marks_existing_binding_assigned() {
    // `for ([x] of values)` assigns to the existing `x` binding. Keeping the
    // initializer as `const x = null` would throw on the first iteration.
    let input = r#"
var x = null;
for ([x] of values) {
    use(x);
}
"#;
    let expected = r#"
let x = null;
for ([x] of values) {
    use(x);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_assignment_destructuring_default_marks_side_effect_assignment() {
    let input = r#"
var flag = false;
var x;
for ([x = flag = true] of values) {
    use(flag, x);
}
"#;
    let expected = r#"
let flag = false;
let x;
for ([x = flag = true] of values) {
    use(flag, x);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_var_destructuring_default_marks_side_effect_assignment() {
    let input = r#"
var initCount = 0;
for (var [x = initCount += 1] of values) {
    use(x);
}
"#;
    let expected = r#"
let initCount = 0;
for (const [x = initCount += 1] of values) {
    use(x);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_var_destructuring_with_duplicate_binding_stays_var() {
    // `var [x, x]` is valid legacy syntax; `const [x, x]` is a SyntaxError.
    let input = r#"
for (var [x, x] of values) {
    use(x);
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
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

#[test]
fn var_inside_try_block_referenced_outside_stays_var() {
    // Babel helpers commonly probe inside try/catch and read the var after the
    // try statement. Converting the try-local `var` to `const` would make the
    // trailing read see a different binding.
    let input = r#"
function isNativeReflectConstruct() {
    try {
        var t = Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function() {}));
    } catch (t) {}
    return !!t;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_inside_try_block_referenced_by_later_closure_stays_var() {
    // Babel's _isNativeReflectConstruct helper memoizes a closure after a
    // try/catch probe. The closure still closes over the function-scoped `var`.
    let input = r#"
function isNativeReflectConstruct() {
    try {
        var t = Boolean.prototype.valueOf.call(Reflect.construct(Boolean, [], function() {}));
    } catch (t) {}
    return function isNativeReflectConstruct() {
        return !!t;
    }();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
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
fn export_named_function_body_before_var_declaration_stays_var() {
    let input = r#"
export function H(B) {
    const cache = B.cache || ti;
    const serializer = B.serializer || tn;
    return { cache, serializer };
}
var tn = function() {
    return JSON.stringify(arguments);
};
var ti = {
    create() { return {}; }
};
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn export_class_before_var_declaration_stays_var() {
    let input = r#"
export class Foo {
    constructor() {
        this.bar = bar;
    }
}
var bar = 42;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn export_var_before_other_var_stays_var() {
    let input = r#"
export var x = y + 1;
var y = 10;
"#;
    let expected = r#"
export const x = y + 1;
var y = 10;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_default_function_body_before_var_declaration_stays_var() {
    // Keep the later var hoisted when an earlier default-exported function
    // closes over it.
    let input = r#"
export default function(value) {
  return read(value);
}
var read = (value) => value;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn export_default_function_member_ref_before_var_declaration_stays_var() {
    let input = r#"
import dep from "dep";
export default function(value) {
  return read.default(value);
}
var read = dep;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn pipeline_export_default_function_callback_before_var_declaration_stays_var() {
    let input = r#"
import dep from "dep";
export default function(target, values) {
  return values.map((value) => read.default(target, value)).filter(Boolean);
}
let alias;
alias = dep;
var read = alias;
"#;
    let expected = r#"
import dep from "dep";
export default function(target, values) {
  return values.map((value) => read.default(target, value)).filter(Boolean);
}
let alias;
alias = dep;
var read = alias;
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn pipeline_cjs_default_function_callback_before_var_declaration_stays_var() {
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", {
    value: true
});
exports.default = function(target, values) {
    return values.map((value) => read.default(target, value)).filter(Boolean);
};
var dep, read = (dep = require("./dep.js")) && dep.__esModule ? dep : {
    default: dep
};
module.exports = exports.default;
"#;
    let expected = r#"
import _dep from "./dep.js";
export default function(target, values) {
    return values.map((value) => read.default(target, value)).filter(Boolean);
};
let dep;
dep = _dep;
var read = dep;
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_default_function_nested_callback_before_var_declaration_stays_var() {
    let input = r#"
import dep from "dep";
export default function(values) {
  return values.map((value) => read.default(value));
}
var read = dep;
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_decl_nested_callback_before_var_declaration_stays_var() {
    let input = r#"
function exported(values) {
  return values.map((value) => read.default(value));
}
var read = dep;
export { exported as default };
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
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
fn for_head_var_referenced_by_prior_function_stays_var() {
    let input = r#"
function read() {
    return arr[i];
}
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn for_head_var_referenced_by_prior_function_expression_stays_var() {
    let input = r#"
var read = function() {
    return arr[i];
};
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let expected = r#"
const read = function() {
    return arr[i];
};
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_head_var_referenced_by_prior_arrow_expression_stays_var() {
    let input = r#"
var read = () => arr[i];
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let expected = r#"
const read = () => arr[i];
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_for_head_var_referenced_by_prior_function_expression_stays_var() {
    let input = r#"
import * as ns from "./define-own-property.js";
export var local1;
var local2;
export { local2 as renamed };
var read = function() {
    return arr[i];
};
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let expected = r#"
import * as ns from "./define-own-property.js";
export let local1;
let local2;
export { local2 as renamed };
const read = function() {
    return arr[i];
};
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);

    let minimal_expected = r#"
import * as ns from "./define-own-property.js";
export var local1;
var local2;
export { local2 as renamed };
const read = function() {
    return arr[i];
};
for (var i = 0, arr = [1]; i < arr.length; i++) {
    read();
}
"#;
    let minimal_output = apply_rule_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&minimal_output, minimal_expected);
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

#[test]
fn var_used_before_decl_inside_same_block_stays_var() {
    // Compound statements currently predeclare all inner vars before scanning
    // the statement body. A same-block read before the declaration still relies
    // on var hoisting and must not become `const`.
    let input = r#"
function f(cond) {
    if (cond) {
        use(x);
        var x = 1;
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_used_before_decl_inside_plain_block_stays_var() {
    let input = r#"
function f() {
    {
        use(x);
        var x = 1;
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_inside_switch_case_referenced_after_switch_stays_var() {
    // `switch` case lists are not BlockStmt nodes, but a `var` declared in a
    // case still hoists to the enclosing function/module scope.
    let input = r#"
function f(tag) {
    switch (tag) {
        case 1:
            var value = compute();
            break;
    }
    return value;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_inside_switch_case_referenced_by_other_case_stays_var() {
    let input = r#"
function recover(kind, state) {
    switch (kind) {
        case 1:
            var status = state.status;
            handle(status);
            break;
        default:
            status = state.fallback;
            handle(status);
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_inside_switch_case_used_only_in_same_case_can_be_const() {
    let input = r#"
function recover(kind, state) {
    switch (kind) {
        case 1:
            var status = state.status;
            handle(status);
            break;
    }
}
"#;
    let expected = r#"
function recover(kind, state) {
    switch (kind) {
        case 1:
            const status = state.status;
            handle(status);
            break;
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_for_counter_captured_by_closure_stays_var() {
    // `var` loop counters are one shared function-scoped binding. Converting
    // this to `let` creates a fresh binding per iteration and changes closures.
    let input = r#"
function f(xs) {
    var fns = [];
    for (var i = 0; i < xs.length; i++) {
        fns.push(function() {
            return i;
        });
    }
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
    return fns[0]();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_inside_loop_body_captured_by_closure_stays_var() {
    // Moving a loop-body `var` to block scope gives each iteration its own
    // binding, while original `var` closures all observe the same binding.
    let input = r#"
function f(xs) {
    var fns = [];
    for (var i = 0; i < xs.length; i++) {
        var value = xs[i];
        fns.push(function() {
            return value;
        });
    }
    return fns[0]();
}
"#;
    let expected = r#"
function f(xs) {
    const fns = [];
    for (let i = 0; i < xs.length; i++) {
        var value = xs[i];
        fns.push(function() {
            return value;
        });
    }
    return fns[0]();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_of_var_captured_by_closure_stays_var() {
    let input = r#"
function f(xs) {
    var fns = [];
    for (var value of xs) {
        fns.push(function() {
            return value;
        });
    }
    return fns[0]();
}
"#;
    let expected = r#"
function f(xs) {
    const fns = [];
    for (var value of xs) {
        fns.push(function() {
            return value;
        });
    }
    return fns[0]();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn for_in_var_captured_by_closure_stays_var() {
    let input = r#"
function f(obj) {
    var fns = [];
    for (var key in obj) {
        fns.push(function() {
            return key;
        });
    }
    return fns[0]();
}
"#;
    let expected = r#"
function f(obj) {
    const fns = [];
    for (var key in obj) {
        fns.push(function() {
            return key;
        });
    }
    return fns[0]();
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn var_duplicate_with_function_param_stays_var() {
    // `var x` and parameter `x` are the same function binding in sloppy-mode
    // inputs. Converting the var to a lexical declaration creates a duplicate
    // binding and changes parse/runtime semantics.
    let input = r#"
function f(x) {
    var x = 1;
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn var_duplicate_with_catch_param_stays_var() {
    // Catch parameters are block-scoped bindings. A `var` with the same name in
    // the catch body is still allowed in legacy inputs, but `let`/`const` would
    // create a duplicate lexical binding.
    let input = r#"
function f() {
    try {
        throw 1;
    } catch (error) {
        var error = 2;
        return error;
    }
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn direct_eval_keeps_scope_vars_as_var() {
    // A direct eval can read or write bindings that are invisible to static
    // assignment collection, so upgrading to `const` is not conservative.
    let input = r#"
function f() {
    var x = 1;
    eval("x = 2");
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn parenthesized_direct_eval_keeps_scope_vars_as_var() {
    let input = r#"
function f() {
    var x = 1;
    (eval)("x = 2");
    return x;
}
"#;
    let expected = r#"
function f() {
    var x = 1;
    eval("x = 2");
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_with_spread_keeps_scope_vars_as_var() {
    let input = r#"
function f(iter) {
    var x = 1;
    eval(...iter, "x = 2");
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn dynamic_direct_eval_keeps_scope_vars_as_var() {
    let input = r#"
function f(source) {
    var x = 1;
    eval(source);
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn nested_direct_eval_argument_keeps_scope_vars_as_var() {
    let input = r#"
function f(source) {
    var x = 1;
    eval("0", eval(source));
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn dynamic_parenthesized_direct_eval_keeps_scope_vars_as_var() {
    let input = r#"
function f(source) {
    var x = 1;
    (eval)(source);
    return x;
}
"#;
    let expected = r#"
function f(source) {
    var x = 1;
    eval(source);
    return x;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn top_level_indirect_eval_keeps_referenced_var() {
    let input = r#"
var count = 0;
(0, eval)("count += 1");
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn function_indirect_eval_does_not_block_local_var() {
    let input = r#"
function f() {
    var count = 0;
    (0, eval)("count += 1");
    return count;
}
"#;
    let expected = r#"
function f() {
    const count = 0;
    (0, eval)("count += 1");
    return count;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn top_level_object_wrapped_eval_keeps_referenced_var() {
    let input = r#"
var count = 0;
Object(eval)("count += 1");
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn direct_eval_hidden_require_does_not_block_unrelated_var() {
    let input = r#"
function f(moduleName) {
    var mod = moduleName === "fs" ? require("fs") : eval("quire".replace(/^/, "re"))(moduleName);
    return mod;
}
"#;
    let expected = r#"
function f(moduleName) {
    const mod = moduleName === "fs" ? require("fs") : eval("quire".replace(/^/, "re"))(moduleName);
    return mod;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn direct_eval_hidden_require_keeps_referenced_var() {
    let input = r#"
function f() {
    var require = getRequire();
    eval("quire".replace(/^/, "re"));
    return require;
}
"#;
    let output = apply_rule(input);
    assert_eq_normalized(&output, input);
}
