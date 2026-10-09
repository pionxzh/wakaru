mod common;

use common::{assert_eq_normalized, render_pipeline, render_rule};
use wakaru_core::rules::UnAssignmentMerging;

fn apply(input: &str) -> String {
    render_rule(input, UnAssignmentMerging::new)
}

fn apply_pipeline(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn splits_two_level_chained_assignment() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    // UnAssignmentMerging splits into: exports.foo = 1; exports.bar = 1;
    // UnEsm then converts to ESM exports
    let input = r#"
exports.foo = exports.bar = 1;
"#;
    // Writes are emitted innermost first, matching the chained form.
    let expected = r#"
export const bar = 1;
export const foo = 1;
"#;

    let output = apply_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn splits_three_level_chained_assignment() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = c = undefined;
"#;
    let expected = r#"
c = undefined;
b = undefined;
a = undefined;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_chain_when_inner_target_reads_outer_ident() {
    let input = r#"
cursor = cursor.next = hook;
"#;
    let expected = r#"
cursor = cursor.next = hook;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_chain_when_outer_target_reads_inner_ident() {
    let input = r#"
cursor.next = cursor = hook;
"#;
    let expected = r#"
cursor.next = cursor = hook;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_member_expression_final_value() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = foo.bar;
"#;
    let expected = r#"
a = b = foo.bar;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_call_expression_final_value() {
    // Reused from packages/unminify/src/transformations/__tests__/un-assignment-merging.spec.ts
    let input = r#"
a = b = fn();
"#;
    let expected = r#"
a = b = fn();
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_regex_literal() {
    // Regex literals create a new object per evaluation; cloning would break
    // identity and shared lastIndex state for g/y regex. (issue #193)
    let input = r#"
a = b = /./g;
"#;
    let expected = r#"
a = b = /./g;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_split_regex_literal_without_flags() {
    let input = r#"
a = b = /pattern/;
"#;
    let expected = r#"
a = b = /pattern/;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn split_keeps_innermost_first_write_order() {
    // `outer` is a `const`: the chained form writes `inner` first and then
    // throws on `outer`. The split must not throw before `inner` is written.
    let input = r#"
const outer = 0;
let inner = 0;
try {
  outer = inner = 1;
} catch (e) {
  console.log(e.name);
}
console.log(inner);
"#;
    let expected = r#"
const outer = 0;
let inner = 0;
try {
  inner = 1;
  outer = 1;
} catch (e) {
  console.log(e.name);
}
console.log(inner);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn split_keeps_setter_order_for_exports_targets() {
    // `exports.a = exports.b = 1` runs the `b` setter before the `a` setter.
    let input = r#"
exports.a = exports.b = 1;
"#;
    let expected = r#"
exports.b = 1;
exports.a = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_chains_that_mirror_a_local_into_an_export() {
    // Babel and TypeScript copy a local into its export in the same chain.
    // The chain is the evidence that the local is the export's storage;
    // UnEsm recovers it as a whole.
    for input in [
        r#"let mode; exports.mode = mode = "on";"#,
        r#"let mode; mode = exports.mode = "on";"#,
        "let mode; module.exports.mode = mode = 1;",
        "let mode; exports.a = exports.b = mode = 1;",
        r#"let mode; function set() { exports.mode = mode = "on"; }"#,
    ] {
        assert_eq_normalized(&apply(input), input);
    }
    // Chains of only export targets or only locals still split.
    assert_eq_normalized(&apply("let a, b; a = b = 1;"), "let a, b; b = 1; a = 1;");
    // A global identifier target is not a local copy.
    assert_eq_normalized(
        &apply("exports.a = global = 1;"),
        "global = 1; exports.a = 1;",
    );
}

#[test]
fn splits_module_exports_alias_chain() {
    // `module` is provided by the CommonJS wrapper, so evaluating
    // `module.exports` cannot throw a ReferenceError.
    let input = r#"
module.exports = exports = fn;
"#;
    let expected = r#"
exports = fn;
module.exports = fn;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn splits_stable_module_exports_property_chain() {
    let input = r#"
module.exports.foo = module.exports.bar = 1;
"#;
    assert_eq_normalized(
        &apply(input),
        "module.exports.bar = 1; module.exports.foo = 1;",
    );
}

#[test]
fn splits_literal_computed_keys_on_commonjs_roots() {
    let input = r#"
exports["a"] = exports[0] = value;
"#;
    let expected = r#"
exports[0] = value;
exports["a"] = value;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_chain_for_this_roots() {
    // `this` throws before `super()` in a derived constructor, and a `this`
    // chain is authored source form anyway.
    let input = r#"
class D extends Object {
  constructor() {
    let inner = 0;
    try {
      this.x = inner = 1;
    } catch {}
    super();
    console.log(inner);
  }
}
this.head = this.tail = null;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_when_identifier_value_could_change_between_writes() {
    // Undeclared global targets may be accessors on the global object whose
    // setter reassigns `value`; the chain reads `value` exactly once.
    let input = r#"
let value = 1;
a = b = value;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn splits_identifier_value_across_resolved_and_commonjs_targets() {
    let input = r#"
let value = 1;
let a, b;
a = b = value;
exports.x = exports.y = value;
module.exports = exports = value;
"#;
    let expected = r#"
let value = 1;
let a, b;
b = value;
a = value;
exports.y = value;
exports.x = value;
exports = value;
module.exports = value;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn splits_literal_value_across_global_identifier_targets() {
    let input = r#"
a = b = undefined;
c = d = 1;
"#;
    let expected = r#"
b = undefined;
a = undefined;
d = 1;
c = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_chain_when_a_local_root_may_be_in_tdz() {
    // `root` is resolved but not yet initialized: the chain throws while
    // evaluating `root.x`, before `inner` is written.
    let input = r#"
let inner = 0;
try {
  root.x = inner = 1;
  let root = {};
} catch {}
console.log(inner);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_for_initialized_local_roots() {
    // An inner setter could reassign `local` between the writes; the rule has
    // no proof against that, so local roots keep the chain.
    let input = r#"
var local = {};
local.a = local.b = 1;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_when_an_outer_root_may_be_undeclared() {
    // `missing.x = inner = 1` throws while evaluating `missing`, before
    // `inner` is written.
    let input = r#"
let inner = 0;
try {
  missing.x = inner = 1;
} catch {}
console.log(inner);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_for_non_commonjs_global_roots() {
    let input = r#"
window.a = window.b = 1;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_for_nested_member_roots() {
    let input = r#"
t.prototype.a = t.prototype.b = fn;
ns.state.a = ns.state.b = 1;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_for_identifier_and_dynamic_computed_keys() {
    let input = r#"
exports[name] = exports.b = true;
object[k()] = object.b = 1;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn keeps_chain_for_call_receivers() {
    let input = r#"
f().x = g().y = 1;
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn nested_export_target_keeps_receiver_before_inner_write() {
    let input = r#"
exports.box = {};
const saved = exports.box;
exports.box.flag = exports.box = 1;
console.log(saved.flag);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn nested_computed_export_target_keeps_receiver_before_inner_write() {
    let input = r#"
module.exports = {};
const saved = module.exports;
module["exports"]["flag"] = module.exports = 1;
console.log(saved.flag);
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn splits_pure_void_without_requiring_remove_void() {
    for root in ["exports", "module.exports"] {
        for value in ["void 0", "void (42)"] {
            for context in [
                "",
                "function dynamic(code) { return eval(code); }",
                "function f(undefined) { return undefined; }",
            ] {
                let input = format!("{root}.a = {root}.b = {value}; {context}");
                let printed_value = value.replace("(42)", "42");
                let expected =
                    format!("{root}.b = {printed_value}; {root}.a = {printed_value}; {context}");
                assert_eq_normalized(&apply(&input), &expected);
            }
        }
    }
    assert_eq_normalized(
        &apply("let a, b; a = b = void 0;"),
        "let a, b; b = void 0; a = void 0;",
    );
}

#[test]
fn nested_export_chain_supports_static_default_and_bracket_keys() {
    assert_eq_normalized(
        &apply("module.exports.default = module.exports[\"value\"] = void 0;"),
        "module.exports[\"value\"] = void 0; module.exports.default = void 0;",
    );
    assert_eq_normalized(
        &apply("module.exports[0] = module.exports[1] = void 0;"),
        "module.exports[1] = void 0; module.exports[0] = void 0;",
    );
    assert_eq_normalized(
        &apply("module.exports.a = module.exports[\"exports\"] = void 0;"),
        "module.exports.a = module.exports[\"exports\"] = void 0;",
    );
}

#[test]
fn keeps_chains_that_need_receiver_or_value_captures() {
    for input in [
        "let obj; obj.a = obj = void 0;",
        "module.exports.a = module.exports = void 0;",
        "module.exports = module.exports.a = void 0;",
        "module.exports.a = module.exports.exports = void 0;",
        "module.exports.a = module.exports.__proto__ = void 0;",
        "exports.a = module.exports.b = void 0;",
        "module.exports[key()] = module.exports.b = void 0;",
        "getObject().a = getObject().b = void 0;",
        "exports.a = exports.b = void sideEffect();",
        "module.exports.a = module.exports.b = makeValue();",
        "const module = { exports: {} }; module.exports.a = module.exports.b = void 0;",
    ] {
        assert_eq_normalized(&apply(input), input);
    }
}

#[test]
fn keeps_chains_in_module_with_with_statement() {
    // A setter on `o` can run between the split writes and change the value
    // the second write reads.
    let input = r#"
with (o) {
    a = b = x;
}
exports.a = exports.b = void 0;
"#;
    assert_eq_normalized(&apply(input), input);
}
