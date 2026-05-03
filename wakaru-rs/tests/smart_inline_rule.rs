mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_rs::{rules::SmartInline, RewriteLevel};

fn apply(input: &str) -> String {
    apply_with_level(input, RewriteLevel::Standard)
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| SmartInline::new(level))
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
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
fn minimal_does_not_inline_single_use_temp_var() {
    let input = r#"
const t = foo;
bar(t);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
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
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
    assert!(
        !output.contains("Ju.forEach"),
        "must not inline e→Ju when Ju is mutated between def and use: {output}"
    );
    assert!(
        output.contains("e.forEach"),
        "temp var e should be preserved: {output}"
    );
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
    assert!(
        output.contains("const n = Nu"),
        "temp var n should be preserved when Nu is mutated: {output}"
    );
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
fn builtin_global_methods_inlined_not_destructured() {
    // Object.defineProperty etc. should be inlined back to member access form,
    // not destructured. Destructuring breaks readability and `this` binding.
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
fn minimal_keeps_builtin_global_alias_inlining() {
    let input = r#"
const a = Object.defineProperty;
a(target, key, desc);
"#;
    let expected = r#"
Object.defineProperty(target, key, desc);
"#;
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, expected);
}

#[test]
fn builtin_global_math_inlined_not_destructured() {
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
fn builtin_global_multi_use_also_inlined() {
    // Even when used multiple times, builtin aliases should be inlined
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
fn builtin_global_accesses_inlined_through_pipeline() {
    // All builtin global aliases are inlined back to Object.X(...) form
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
    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn builtin_alias_inlined_in_function_scope() {
    // Math.floor/Math.ceil aliases declared inside a function body should be inlined
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
    let output = apply_pipeline(input);
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
    assert!(
        output.contains("const r = M") || output.contains("const r = initial"),
        "should keep the temp var, got:\n{}",
        output
    );
    assert!(
        !output.contains("M = M"),
        "must not produce M = M, got:\n{}",
        output
    );
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
    let output = apply(input);
    // u should be inlined since e is not mutated after const u = e
    assert!(
        !output.contains("const u = "),
        "should inline u, got:\n{}",
        output
    );
    assert!(
        output.contains("console.log(e)"),
        "u should be replaced with e, got:\n{}",
        output
    );
}
