mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::{decompile, rules::SmartInline, DecompileOptions, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |unresolved_mark| {
        SmartInline::new_with_mark(level, unresolved_mark)
    })
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

fn apply_pipeline_with_level(input: &str, level: RewriteLevel) -> String {
    decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level,
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
    .code
}

#[test]
fn inline_single_use_temp_var() {
    let input = r#"
const t = foo;
bar(t);
"#;
    let expected = r#"
bar(foo);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_decl_keeps_statement_run_order() {
    let input = r#"
const first = source.first;
const second = source.second;
export class ExportedClass {}
decorate(ExportedClass.prototype);
"#;
    let expected = r#"
const { first, second } = source;
export class ExportedClass {
}
decorate(ExportedClass.prototype);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_inline_single_use_temp_var() {
    let input = r#"
const t = foo;
bar(t);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_forwards_adjacent_assignment_alias() {
    let input = r#"
async function loadValue(id) {
    let h;
    let value;
    h = cached ?? await load_backup(id);
    value = h;
    return value;
}
"#;
    let expected = r#"
async function loadValue(id) {
    let h;
    let value;
    value = cached ?? await load_backup(id);
    return value;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_preserves_adjacent_assignment_alias() {
    let input = r#"
async function loadValue(id) {
    let h;
    let value;
    h = cached ?? await load_backup(id);
    value = h;
    return value;
}
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_forward_assignment_alias_when_temp_read_later() {
    let input = r#"
async function loadValue(id) {
    let h;
    let value;
    h = await load_backup(id);
    value = h;
    observe(h);
    return value;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_forward_assignment_alias_when_temp_captured() {
    let input = r#"
async function loadValue(id) {
    let h;
    let value;
    const read = () => h;
    h = await load_backup(id);
    value = h;
    return read();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_forward_assignment_alias_with_direct_eval() {
    let input = r#"
async function loadValue(id) {
    let h;
    let value;
    h = await load_backup(id);
    value = h;
    eval("h");
    return value;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_forward_assignment_alias_from_initialized_temp() {
    let input = r#"
async function loadValue(id) {
    let h = cached;
    let value;
    h = await load_backup(id);
    value = h;
    return value;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_forward_assignment_alias_to_unresolved_target() {
    let input = r#"
async function loadValue(id) {
    let h;
    h = await load_backup(id);
    value = h;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_multi_use_temp_var() {
    // t is used twice — should NOT be inlined
    let input = r#"
const t = foo;
bar(t);
baz(t);
"#;
    let expected = r#"
const t = foo;
bar(t);
baz(t);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_literal_const() {
    // Literal constants are intentionally named — keep them
    let input = r#"
const n = 42;
process(n);
"#;
    let expected = r#"
const n = 42;
process(n);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_into_nested_function() {
    // t used inside nested fn — top-level count is 0, shouldn't inline
    // ArrowFunction rule converts the function expression to an arrow function.
    let input = r#"
const t = foo;
export const fn2 = function() { return t; };
"#;
    let expected = r#"
const t = foo;
export const fn2 = () => t;
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_when_temp_is_captured_by_nested_function() {
    let input = r#"
function restoreLater(value) {
    const target = value;
    const oldToJSON = target.toJSON;
    if (oldToJSON) {
        delete target.toJSON;
        return () => {
            target.toJSON = oldToJSON;
        };
    }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_use_above_declaration() {
    // A pre-declaration const read would throw via TDZ at runtime. Inlining the
    // initializer would erase that observable failure.
    let input = r#"
consume(t);
const t = foo;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_when_source_binding_declared_after_temp() {
    // The temp read throws before `source` is initialized. Inlining would move
    // the read to the later use site and erase that TDZ failure.
    let input = r#"
const t = source;
const source = value;
consume(t);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_when_source_binding_used_above_own_declaration() {
    let input = r#"
observe(source);
const source = value;
const t = source;
consume(t);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_when_source_binding_declared_in_for_init() {
    let input = r#"
for (let source = value; active; source = next()) {
    const t = source;
    consume(t);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_ident_alias_used_in_loop_body() {
    // `t` captures source once before the loop. Replacing it with `source`
    // would re-read source on every iteration; `source` is not a const binding.
    let input = r#"
let source = value;
const t = source;
for (let i = 0; i < 3; i++) {
    consume(t);
    mutateSource();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn inline_loop_alias_when_source_is_prior_const() {
    // Re-reading a prior local const binding inside the loop is stable, even
    // when other calls happen in the loop body.
    let input = r#"
const source = value;
const t = source;
for (let i = 0; i < 3; i++) {
    consume(t);
    observe();
}
"#;
    let expected = r#"
const source = value;
for (let i = 0; i < 3; i++) {
    consume(source);
    observe();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_property_destructuring() {
    // aliases a/b/c are ≤2 chars → SmartRename converts them to shorthand x/y/z
    let input = r#"
const a = obj.x;
const b = obj.y;
const c = obj.z;
"#;
    let expected = r#"
const { x, y, z } = obj;
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_array_destructuring() {
    let input = r#"
const a = arr[0];
const b = arr[1];
const c = arr[2];
"#;
    let expected = r#"
const [a, b, c] = arr;
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_group_array_destructuring() {
    let input = r#"
const a = arr[0];
const b = arr[1];
const c = arr[2];
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn minimal_does_not_group_array_destructuring() {
    let input = r#"
const a = arr[0];
const b = arr[1];
const c = arr[2];
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn group_array_destructuring_with_holes() {
    let input = r#"
const a = arr[0];
const c = arr[2];
"#;
    let expected = r#"
const [a, , c] = arr;
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_use_state_tuple_reads() {
    let input = r#"
const { useState } = React;
function Component() {
    const pair = useState(0);
    const count = pair[0];
    const setCount = pair[1];
    return count;
}
"#;
    let expected = r#"
const { useState } = React;
function Component() {
    const [count, setCount] = useState(0);
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_member_use_state_tuple_reads() {
    let input = r#"
function Component() {
    const pair = React.useState(false);
    const open = pair[0];
    const setOpen = pair[1];
    return open;
}
"#;
    let expected = r#"
function Component() {
    const [open, setOpen] = React.useState(false);
    return open;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_length_two_helper_wrapped_use_state_tuple_reads() {
    let input = r#"
function Component() {
    const pair = helper(React.useState(false), 2);
    const open = pair[0];
    const setOpen = pair[1];
    return open;
}
"#;
    let expected = r#"
function Component() {
    const [open, setOpen] = helper(React.useState(false), 2);
    return open;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_use_state_assignment_tuple_reads() {
    let input = r#"
function Component() {
    let count;
    let setCount;
    const pair = React.useState(false);
    count = pair[0];
    setCount = pair[1];
    return count;
}
"#;
    let expected = r#"
function Component() {
    const [count, setCount] = React.useState(false);
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_use_state_ref_assignment_tuple_reads() {
    let input = r#"
function Component() {
    let pair;
    let count;
    let setCount;
    pair = React.useState(false);
    count = pair[0];
    setCount = pair[1];
    return count;
}
"#;
    let expected = r#"
function Component() {
    const [count, setCount] = React.useState(false);
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_folds_use_state_nested_assignment_tuple_reads() {
    let input = r#"
function Component() {
    let pair;
    let count;
    let setCount;
    count = (pair = React.useState(false))[0];
    setCount = pair[1];
    return count;
}
"#;
    let expected = r#"
function Component() {
    const [count, setCount] = React.useState(false);
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn use_state_assignment_tuple_keeps_prior_read() {
    let input = r#"
function Component() {
    let pair;
    let count;
    let setCount;
    console.log(pair);
    count = (pair = React.useState(false))[0];
    setCount = pair[1];
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn use_state_assignment_tuple_keeps_later_write() {
    let input = r#"
function Component() {
    let pair;
    let count;
    let setCount;
    count = (pair = React.useState(false))[0];
    setCount = pair[1];
    count = 1;
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_does_not_fold_local_function_named_use_state() {
    let input = r#"
function useState(value) {
    return {
        0: value,
        1: function () {}
    };
}
function Component() {
    const pair = useState(0);
    const count = pair[0];
    const setCount = pair[1];
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_does_not_fold_nested_destructured_use_state_property() {
    let input = r#"
const { useState: { value } } = React;
function Component() {
    const pair = value(0);
    const count = pair[0];
    const setCount = pair[1];
    return count;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn use_state_tuple_keeps_temp_when_reused() {
    let input = r#"
const { useState } = React;
function Component() {
    const pair = useState(0);
    const count = pair[0];
    const setCount = pair[1];
    console.log(pair);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_group_single_property_access() {
    // Only one access — not worth destructuring
    let input = r#"
const a = obj.x;
"#;
    let expected = r#"
const a = obj.x;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_group_single_index_access() {
    let input = r#"
const a = arr[0];
"#;
    let expected = r#"
const a = arr[0];
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn group_property_shorthand_after_rename() {
    // When alias == prop key name, smart-rename converts to shorthand
    let input = r#"
const x = obj.x;
const y = obj.y;
"#;
    let expected = r#"
const { x, y } = obj;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

// ============================================================
// Zero-param arrow wrapper inlining (require.n / webpack4 pattern)
// ============================================================

#[test]
fn inline_arrow_wrapper_into_nested_function() {
    // `const o = () => r` used once inside a nested function should be inlined
    // globally across the function boundary, and the resulting (() => r)() call
    // should collapse to just `r` via the second UnIife pass.
    let input = r#"
const o = () => r;
export function foo() {
    return o();
}
"#;
    let expected = r#"
export function foo() {
    return r;
}
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_arrow_wrapper_at_all_use_sites() {
    // Arrow wrappers are inlined everywhere regardless of use count —
    // they are pure aliases with no semantic value (e.g. require.n wrappers).
    let input = r#"
const o = () => r;
export function foo() { return o(); }
export function bar() { return o(); }
"#;
    let expected = r#"
export function foo() { return r; }
export function bar() { return r; }
"#;
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn arrow_wrapper_dot_a_accessor_stays_as_is() {
    let input = r#"
const o = () => r;
function foo() {
    return o.a;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn known_bug_arrow_wrapper_dot_a_non_webpack_shape_not_inlined() {
    let input = r#"
const o = () => r;
console.log(o.a);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_when_source_ident_mutated_between_def_and_use() {
    // Regression: const e = Ju; Ju = null; e.forEach(...)
    // SmartInline inlined e → Ju, producing Ju.forEach(null) — a null dereference.
    // The temp var exists to capture Ju's value before mutation.
    let input = r#"
if (Ju !== null) {
    const e = Ju;
    Ju = null;
    e.forEach((v, k) => { process(k, v); });
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn no_inline_ident_snapshot_across_side_effectful_call() {
    let input = r#"
let foo = 1;
function mutate() {
    foo = 2;
}
const t = foo;
mutate();
returnValue(t);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn inline_when_only_shadowed_source_ident_is_mutated() {
    let input = r#"
let foo = value;
const t = foo;
function mutate(foo) {
    foo = other;
}
consume(t);
"#;
    let expected = r#"
let foo = value;
function mutate(foo) {
    foo = other;
}
consume(foo);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_when_source_ident_reassigned_in_finally() {
    // Pattern: var n = Nu; Nu = ku; ... finally { (Nu = n) === xu }
    // n captures old Nu before mutation — must not inline to Nu
    let input = r#"
const n = Nu;
Nu = ku;
try { doWork(); } finally { Nu = n; check(Nu); }
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn inline_still_works_when_source_not_mutated() {
    // Normal case: source ident is never mutated, inlining is safe
    let input = r#"
const t = foo;
bar(t);
"#;
    let expected = r#"
bar(foo);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_ident_snapshot_across_unrelated_local_assignment() {
    let input = r#"
function f() {
  const t = foo;
  flag = true;
  return t;
}
"#;
    let expected = r#"
function f() {
  flag = true;
  return foo;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_inlines_builtin_global_member_aliases() {
    // stable_builtins: standard mode assumes Object.defineProperty and
    // Object.getOwnPropertyNames are not patched between alias capture and use.
    let input = r#"
const a = Object.defineProperty;
const b = Object.getOwnPropertyNames;
a(target, key, desc);
b(source);
"#;
    let expected = r#"
Object.defineProperty(target, key, desc);
Object.getOwnPropertyNames(source);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_preserves_builtin_global_member_aliases() {
    let input = r#"
const a = Object.defineProperty;
a(target, key, desc);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn aggressive_keeps_builtin_global_member_alias_inlining() {
    let input = r#"
const a = Object.defineProperty;
const b = Object.getOwnPropertyNames;
a(target, key, desc);
b(source);
"#;
    let expected = r#"
Object.defineProperty(target, key, desc);
Object.getOwnPropertyNames(source);
"#;
    let output = apply_with_level(input, RewriteLevel::Aggressive);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_inlines_builtin_global_ident_aliases() {
    // stable_builtins: standard mode opts into re-reading Object/TypeError at
    // the use site instead of preserving the captured alias value.
    let input = r#"
const O = Object;
const E = TypeError;
const x = O.create(null);
throw new E("bad");
"#;
    let expected = r#"
const x = Object.create(null);
throw new TypeError("bad");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_remove_builtin_alias_with_blocked_shorthand_use() {
    let input = r#"
const E = TypeError;
const errors = { E };
throw new E("bad");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_inlines_builtin_global_math_member_aliases() {
    let input = r#"
const a = Math.ceil;
const b = Math.floor;
a(1.5);
b(2.5);
"#;
    let expected = r#"
Math.ceil(1.5);
Math.floor(2.5);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_inline_member_alias_from_local_builtin_name() {
    let input = r#"
let Object = customObject;
const a = Object.defineProperty;
a(t1, k1, d1);
a(t2, k2, d2);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_does_not_inline_member_alias_from_imported_builtin_name() {
    let input = r#"
import { Object } from "custom";
const a = Object.defineProperty;
a(t1, k1, d1);
a(t2, k2, d2);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_does_not_inline_bare_alias_from_local_builtin_name() {
    let input = r#"
const TypeError = CustomError;
const E = TypeError;
throw new E("bad");
handle(E);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_builtin_global_multi_use_also_inlined() {
    // Even when used multiple times, builtin aliases can be inlined under
    // stable_builtins.
    let input = r#"
const a = Object.defineProperty;
a(t1, k1, d1);
a(t2, k2, d2);
"#;
    let expected = r#"
Object.defineProperty(t1, k1, d1);
Object.defineProperty(t2, k2, d2);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_inline_environment_global_member_aliases() {
    let input = r#"
const q = document.querySelector;
q(".one");
q(".two");
const cwd = process.cwd;
cwd();
cwd();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_inlines_newer_standard_builtin_alias_roots() {
    let input = r#"
const B = BigInt;
B(1);
B(2);
const add = Atomics.add;
add(i32, 0, 1);
add(i32, 0, 2);
const W = WeakRef;
new W(target);
new W(otherTarget);
const F = FinalizationRegistry;
new F(cleanup);
new F(otherCleanup);
"#;
    let expected = r#"
BigInt(1);
BigInt(2);
Atomics.add(i32, 0, 1);
Atomics.add(i32, 0, 2);
new WeakRef(target);
new WeakRef(otherTarget);
new FinalizationRegistry(cleanup);
new FinalizationRegistry(otherCleanup);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_does_not_destructure_newer_standard_builtin_roots() {
    let input = r#"
const add = Atomics.add;
const wait = Atomics.wait;
const deref = WeakRef.prototype.deref;
const cleanup = FinalizationRegistry.prototype.cleanupSome;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_does_not_inline_broad_builtin_global_function_aliases() {
    let input = r#"
const p = parseInt;
p("1", 10);
p("2", 10);
const d = decodeURI;
d(url);
d(nextUrl);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn standard_builtin_global_accesses_inlined_through_pipeline() {
    // All builtin global aliases are inlined back to Object.X(...) form in
    // standard mode.
    let input = r#"
const i = Object.defineProperty;
const c = Object.getPrototypeOf;
const s = c && c(Object);
i(target, key, desc);
"#;
    let expected = r#"
const s = Object.getPrototypeOf && Object.getPrototypeOf(Object);
Object.defineProperty(target, key, desc);
"#;
    let output = apply_pipeline_with_level(input, RewriteLevel::Standard);
    assert_eq_normalized(&output, expected);
}

#[test]
fn standard_builtin_alias_inlined_in_function_scope() {
    // Math.floor/Math.ceil aliases declared inside a function body can be
    // inlined in standard mode.
    let input = r#"
const x = (function() {
    const Math_ceil = Math.ceil;
    const Math_floor = Math.floor;
    function compute(n) {
        return Math_floor(n) + Math_ceil(n * 2);
    }
    return compute(3.5);
})();
"#;
    let expected = r#"
const x = (()=>{
    function compute(n) {
        return Math.floor(n) + Math.ceil(n * 2);
    }
    return compute(3.5);
})();
"#;
    let output = apply_pipeline_with_level(input, RewriteLevel::Standard);
    assert_eq_normalized(&output, expected);
}

#[test]
fn no_inline_when_source_mutated_after_def_in_try_finally() {
    // Save/restore pattern: const r = M; try { mutate M } finally { M = r; }
    // Must NOT inline because M is mutated inside the try block.
    let input = r#"
const M = initial;
const r = M;
try {
    M = newValue;
} finally {
    M = r;
}
"#;
    let output = apply_pipeline(input);
    // r must be preserved — inlining would produce M = M (self-assignment)
    insta::assert_snapshot!(output);
}

#[test]
fn inline_when_source_mutated_only_before_def() {
    // Mutation happens before def, not after — safe to inline.
    let input = r#"
let e = first;
e = second;
const u = e;
console.log(u);
"#;
    let expected = r#"
let e = first;
e = second;
console.log(e);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
