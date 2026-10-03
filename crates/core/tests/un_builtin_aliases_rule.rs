mod common;

use common::{assert_eq_normalized, render, render_pipeline_between, render_rule};
use wakaru_core::rules::UnBuiltinAliases;

fn apply(input: &str) -> String {
    render_rule(input, UnBuiltinAliases::new)
}

#[test]
fn inlines_module_var_builtin_member_aliases() {
    let input = r#"
var e = Object.freeze;
var r = Object.defineProperty;
use(e(r(strings, "raw", { value: e(raws) })));
"#;
    let expected = r#"
use(Object.freeze(Object.defineProperty(strings, "raw", {
    value: Object.freeze(raws)
})));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inlines_module_const_builtin_member_aliases() {
    let input = r#"
const e = Object.freeze;
use(e(value));
"#;
    let expected = r#"
use(Object.freeze(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn public_exported_builtin_alias_remains_declared() {
    let input = r#"
var defineProperty = Object.defineProperty;
export { defineProperty };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_used_before_initializer() {
    let input = r#"
use(e);
var e = Object.freeze;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_reassigned_var_alias() {
    let input = r#"
var e = Object.freeze;
e = other;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_when_direct_eval_can_observe_binding() {
    let input = r#"
var e = Object.freeze;
eval("e");
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_alias_from_local_builtin_shadow() {
    let input = r#"
const Object = fake;
var e = Object.freeze;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_redeclared_with_non_alias_init() {
    let input = r#"
var e = Object.freeze;
use(e(value));
var e = getPolyfill();
use2(e);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_redeclaring_non_alias_binding() {
    let input = r#"
var e = getPolyfill();
use(e);
var e = Object.freeze;
use2(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_mutated_by_update_expression() {
    let input = r#"
var e = Object.freeze;
e++;
use(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn preserves_var_alias_removed_with_delete() {
    let input = r#"
var e = Object.freeze;
use(delete e);
use2(e(value));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn alias_named_by_export_specifier_remains_declared() {
    // `export { o }` can only name a binding, so the alias declaration has to
    // stay even though every expression use could be inlined.
    let input = r#"
var o = Object.create;
var d = Object.defineProperty;
function f(q) {
    return d(o(q), "x", { value: 1 });
}
export { o, f };
"#;
    let expected = r#"
var o = Object.create;
function f(q) {
    return Object.defineProperty(o(q), "x", { value: 1 });
}
export { o, f };
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn alias_call_inside_a_computed_object_key_is_inlined() {
    let input = r#"
const o = Object.keys;
const m = { [o(x)[0]]: 1 };
o(y);
export { m };
"#;
    let output = apply(input);
    assert!(output.contains("[Object.keys(x)[0]]: 1"), "{output}");
    assert!(output.contains("Object.keys(y);"), "{output}");
    assert!(!output.contains("const o ="), "{output}");
    assert!(!output.contains("o(x)"), "{output}");
}

#[test]
fn preserves_const_alias_when_module_has_dynamic_scope() {
    // The `var` path already rejected dynamic scope; `const`/`let` aliases
    // are the same hazard: inlining reads `Object` as the global at a site
    // where `with` or a direct eval may have bound that name.
    for hazard in ["eval(code);", "with (scope) { observe(); }"] {
        let input = format!("const e = Object.freeze;\n{hazard}\nuse(e(value));\n");
        assert_eq_normalized(&apply(&input), &input);
    }
}

#[test]
fn alias_returned_by_export_getter_remains_declared() {
    // UnEsm turns each getter that returns a local into an ESM export of that
    // binding. Inlining the alias first would leave the getter returning the
    // global, which ESM cannot export.
    for getters in [
        "require.d(exports, { U: () => i });",
        "require.d(exports, \"U\", function() { return i; });",
        "Object.defineProperty(exports, \"U\", { enumerable: true, get: function() { return i; } });",
    ] {
        let input = format!("{getters}\nconst i = console;\n");
        assert_eq_normalized(&apply(&input), &input);
    }
}

#[test]
fn alias_read_inside_export_getter_expression_is_still_inlined() {
    // Only a getter that returns the binding whole names it as an export.
    let input = r#"
require.d(exports, { U: () => i.log });
const i = console;
"#;
    let expected = r#"
require.d(exports, { U: () => console.log });
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn export_getter_of_builtin_alias_becomes_a_declared_export() {
    let input = r#"
require.d(exports, { U: () => i, V: () => j });
const i = console;
const j = JSON;
"#;
    let output = render_pipeline_between(input, "UnBuiltinAliases", "UnEsm");
    assert!(
        !output.contains("export { console") && !output.contains("export { JSON"),
        "an export must name a declared binding:\n{output}"
    );
    assert!(
        output.contains("console") && output.contains("JSON"),
        "{output}"
    );
    assert!(!output.contains("require.d"), "{output}");
}

#[test]
fn export_getter_loop_of_builtin_alias_becomes_a_declared_export() {
    // The inlined `require.d` loop reaches UnBuiltinAliases only after the
    // earlier pipeline rules normalize it, so run the whole pipeline.
    let input = r#"
((t, e) => { for (var n in e) Object.defineProperty(t, n, { enumerable: true, get: e[n] }); })(exports, { U: () => i });
const i = console;
"#;
    let output = render(input);
    assert!(!output.contains("export { console"), "{output}");
    assert!(output.contains("console"), "{output}");
    assert!(!output.contains("defineProperty"), "{output}");
}
