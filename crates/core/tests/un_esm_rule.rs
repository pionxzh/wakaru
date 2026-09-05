mod common;

use common::{
    assert_eq_normalized, render_pipeline, render_pipeline_until,
    render_pipeline_until_with_filename, render_pipeline_until_with_level,
};
use wakaru_core::{validate_output_modules, OutputFindingKind, RewriteLevel};

// Stop before DeadImports (the final cleanup pass) so that synthetic inputs
// with unused specifiers don't get stripped — these tests exercise UnEsm's
// shape, not downstream dead-code elimination.
fn apply(input: &str) -> String {
    render_pipeline_until(input, "SmartRename")
}

fn apply_with_level(input: &str, level: RewriteLevel) -> String {
    render_pipeline_until_with_level(input, "SmartRename", level)
}

#[test]
fn bare_require_to_import() {
    // require('side-effect') → import 'side-effect'
    let input = "require('side-effect');";
    let expected = r#"import "side-effect";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn minimal_does_not_convert_bare_require_to_import() {
    let input = "require('side-effect');";
    let output = apply_with_level(input, RewriteLevel::Minimal);
    assert_eq_normalized(&output, input);
}

#[test]
fn local_require_binding_not_converted_to_import() {
    let input = r#"
function require(x) {
  return x;
}
var foo = require("foo");
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn default_require_to_import() {
    let input = "var foo = require('foo');";
    let expected = r#"import foo from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn local_self_require_stays_at_the_commonjs_boundary() {
    let input = r#"
var self = require("./module-1.js");
for (var key in self) globalThis[key] = self[key];
"#;
    let expected = r#"
var self = require("./module-1.js");
for (var key in self) {
  globalThis[key] = self[key];
}
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn local_self_require_keeps_the_complete_commonjs_surface() {
    let input = r#"
var self = require("./module-1.js");
var dependency = require("./module-2.js");
exports.value = dependency.value;
consume(self, dependency);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a self-requiring module must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn linkable_default_self_import_keeps_existing_recovery() {
    let input = r#"
var self = require("./module-1.js");
module.exports = function api() {};
consume(self);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert!(
        output.contains(r#"import self from "./module-1.js""#)
            && output.contains("export default")
            && output.contains("function api()")
            && !output.contains("require("),
        "a self-cycle with a real default surface should retain existing recovery:\n{output}"
    );
}

#[test]
fn named_self_surface_uses_the_proven_namespace_boundary() {
    let input = r#"
var self = require("./module-1.js");
exports.value = function value() {
  return self.value;
};
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert!(
        output.contains(r#"import * as self from "./module-1.js""#)
            && output.contains("export const value")
            && output.contains("return self.value")
            && !output.contains("require("),
        "a static read from a proven named self surface should use the existing namespace proof:\n{output}"
    );
}

#[test]
fn same_basename_in_another_directory_is_not_a_self_require() {
    let input = r#"
var sibling = require("../module-1.js");
consume(sibling);
"#;
    let expected = r#"
import sibling from "../module-1.js";
consume(sibling);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "nested/module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn shadowed_self_require_spelling_does_not_block_unesm() {
    let input = r#"
function inspect(require) {
  return require("./module-1.js");
}
var dependency = require("./module-2.js");
consume(inspect, dependency);
"#;
    let expected = r#"
import dependency from "./module-2.js";
function inspect(require) {
  return require("./module-1.js");
}
consume(inspect, dependency);
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, expected);
}

#[test]
fn multi_declarator_require_to_imports() {
    let input = r#"
var react = require("react"), jsx = require("react/jsx-runtime"), ctx = react.createContext(null);
"#;
    let expected = r#"
import react from "react";
import jsx from "react/jsx-runtime";
const ctx = react.createContext(null);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exported_require_to_import_and_export_specifier() {
    let input = r#"
export const dep = require("./dep.js");
export const value = dep.value;
"#;
    let expected = r#"
import dep from "./dep.js";
export { dep };
export const value = dep.value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mixed_exported_require_declaration_preserves_other_exports() {
    let input = r#"
export const local = 1, dep = require("./dep.js"), value = dep.value;
"#;
    let expected = r#"
import dep from "./dep.js";
export const local = 1;
export { dep };
export const value = dep.value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn destructure_require_to_named_import() {
    // var { a, b: c } = require('foo')
    // UnEsm produces: import { a, b as c } from "foo"
    // UnImportRename then renames the alias `c` back to the imported name `b`
    let input = "var { a, b: c } = require('foo');";
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn property_require_to_named_import() {
    // UnEsm produces: import { baz as foo } from "bar"
    // UnImportRename then renames `foo` to `baz` (the imported name)
    let input = "var foo = require('bar').baz;";
    let expected = r#"import { baz } from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn default_property_require() {
    let input = "var foo = require('bar').default;";
    let expected = r#"import foo from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn webpack_default_getter_collapses_to_import() {
    let input = r#"
var r = require('foo');
var o = () => r && r.__esModule ? r.default : r;
function load() {
  return o();
}
"#;
    let expected = r#"
import r from "foo";
function load() {
  return r;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn merge_same_source_imports() {
    let input = r#"
var foo = require('foo');
var { bar } = require('foo');
"#;
    let expected = r#"import foo, { bar } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn commonjs_object_keys_reexport_loop_becomes_export_star() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  key !== "default" && key !== "__esModule" &&
    (key in exports && exports[key] === source[key] ||
      (exports[key] = source[key]));
});
"#;
    let expected = r#"export * from "./source.js";"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn commonjs_object_keys_reexport_loop_with_extra_binding_use_is_unchanged() {
    let input = r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
  key !== "default" && key !== "__esModule" &&
    (key in exports && exports[key] === source[key] ||
      (exports[key] = source[key]));
});
observe(source);
"#;
    let output = apply(input);
    assert!(
        !output.contains("export * from"),
        "an escaped require binding must keep the re-export loop:\n{output}"
    );
    assert!(output.contains("Object.keys(source)"));
}

#[test]
fn multiple_defaults_separate_imports() {
    // Two require() calls for the same module produce the same value;
    // ImportDedup canonicalizes to the first local binding.
    let input = r#"
var foo = require('foo');
var bar = require('foo');
"#;
    let expected = r#"
import foo from "foo";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn mutable_require_binding_uses_a_separate_import_binding() {
    let input = r#"
var dependency = require("./dependency.js");
dependency = replacement;
consume(dependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
let dependency = _dependency;
dependency = replacement;
consume(dependency);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);

    let findings = validate_output_modules(&[
        ("entry.js".to_string(), output),
        (
            "dependency.js".to_string(),
            "export default {};".to_string(),
        ),
    ]);
    assert!(
        !findings
            .iter()
            .any(|finding| finding.kind == OutputFindingKind::AssignToImport),
        "mutable local must not write to the synthesized import: {findings:#?}"
    );
}

#[test]
fn written_const_require_binding_preserves_its_authored_contract() {
    let input = r#"
const dependency = require("./dependency.js");
dependency = replacement;
consume(dependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
const dependency = _dependency;
dependency = replacement;
consume(dependency);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn nested_write_to_require_binding_stays_on_the_local() {
    let input = r#"
var dependency = require("./dependency.js");
function replaceDependency(next) {
    dependency = next;
}
consume(dependency, replaceDependency);
"#;
    let expected = r#"
import _dependency from "./dependency.js";
let dependency = _dependency;
function replaceDependency(next) {
    dependency = next;
}
consume(dependency, replaceDependency);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn update_to_require_binding_stays_on_the_local() {
    let input = r#"
var counter = require("./counter.js");
counter++;
consume(counter);
"#;
    let expected = r#"
import _counter from "./counter.js";
let counter = _counter;
counter++;
consume(counter);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_destructured_require_uses_a_separate_import_binding() {
    let input = r#"
var { value } = require("./dependency.js");
value = replacement;
consume(value);
"#;
    let expected = r#"
import _value from "./dependency.js";
let { value } = _value;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_named_property_require_uses_a_separate_import_binding() {
    let input = r#"
var value = require("./dependency.js").value;
value = replacement;
consume(value);
"#;
    let expected = r#"
import { value as value_1 } from "./dependency.js";
let value = value_1;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn mutable_default_property_require_uses_a_separate_import_binding() {
    let input = r#"
var value = require("./dependency.js").default;
value = replacement;
consume(value);
"#;
    let expected = r#"
import _value from "./dependency.js";
let value = _value;
value = replacement;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn exported_mutable_require_retains_the_local_export() {
    let input = r#"
export let dependency = require("./dependency.js");
dependency = replacement;
"#;
    let expected = r#"
import _dependency from "./dependency.js";
export let dependency = _dependency;
dependency = replacement;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn multiple_mutable_requires_share_the_canonical_import() {
    let input = r#"
var first = require("./dependency.js");
var second = require("./dependency.js");
first = replacementOne;
second = replacementTwo;
consume(first, second);
"#;
    let expected = r#"
import _first from "./dependency.js";
let first = _first;
let second = _first;
first = replacementOne;
second = replacementTwo;
consume(first, second);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn module_exports_default() {
    let input = "module.exports = 1;";
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn called_module_exports_assignment_preserves_export_and_call() {
    let input = r#"(module.exports = factory)(argument);"#;
    let expected = r#"
const _default = factory;
export default _default;
_default(argument);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_evaluates_rhs_once() {
    let input = r#"(module.exports = createFactory())(argument);"#;
    let expected = r#"
const _default = createFactory();
export default _default;
_default(argument);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_as_member_receiver_preserves_export_and_call() {
    let input = r#"
(module.exports = factory)("versions", []).push(record);
"#;
    let expected = r#"
const _default = factory;
export default _default;
_default("versions", []).push(record);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_module_exports_assignment_receiver_chain_evaluates_rhs_once() {
    let input = r#"
(module.exports = createFactory())(argument).result.consume();
"#;
    let expected = r#"
const _default = createFactory();
export default _default;
_default(argument).result.consume();
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn called_local_module_exports_assignment_is_not_transformed() {
    let input = r#"
const module = { exports: null };
(module.exports = factory)(argument);
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_assignment_in_single_var_initializer_preserves_binding() {
    let input = r#"
var value = module.exports = createValue();
use(value);
"#;
    let expected = r#"
const value = createValue();
export default value;
use(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn module_exports_assignment_in_single_var_initializer_evaluates_rhs_once() {
    let input = r#"
var value = module.exports = makeValue(sideEffect());
consume(value);
"#;
    let output = apply(input);
    assert_eq!(output.matches("makeValue(sideEffect())").count(), 1);
    assert!(output.contains("export default value;"));
}

#[test]
fn local_module_exports_assignment_in_var_initializer_is_not_transformed() {
    let input = r#"
const module = { exports: null };
var value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_assignment_in_split_multi_var_initializer_preserves_order() {
    let input = r#"
var before = observeBefore(), value = module.exports = createValue(), after = observeAfter();
"#;
    let expected = r#"
var before = observeBefore();
var value = createValue();
export default value;
var after = observeAfter();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), expected);
}

#[test]
fn chained_local_module_exports_assignment_preserves_order() {
    let input = r#"
let value;
value = module.exports = createValue();
consume(value);
"#;
    let expected = r#"
let value;
const _default = createValue();
export default _default;
value = _default;
consume(value);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn chained_local_module_exports_assignment_evaluates_rhs_once() {
    let input = r#"
let value;
value = module.exports = makeValue(sideEffect());
"#;
    let output = apply(input);
    assert_eq!(output.matches("makeValue(sideEffect())").count(), 1);
    assert!(output.contains("export default _default;"));
    assert!(output.contains("value = _default;"));
}

#[test]
fn chained_local_module_exports_assignment_with_local_module_is_not_transformed() {
    let input = r#"
const module = { exports: null };
let value;
value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_unresolved_module_exports_assignment_is_not_transformed() {
    let input = r#"value = module.exports = createValue();"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_const_module_exports_assignment_is_not_transformed() {
    let input = r#"
const value = initialValue;
value = module.exports = createValue();
"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn chained_member_module_exports_assignment_is_not_transformed() {
    let input = r#"holder.value = module.exports = createValue();"#;
    assert_eq_normalized(&render_pipeline_until(input, "UnEsm"), input);
}

#[test]
fn module_exports_default_ident_not_affected() {
    // CJS module.exports = ident still produces export default (the declaration
    // is before the export, so no TDZ issue).
    let input = r#"
const o = { foo: 1 };
module.exports = o;
"#;
    let expected = r#"
const o = { foo: 1 };
export default o;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn stable_default_binding_replaces_later_commonjs_mirror_reads() {
    let input = r#"
const api = () => "ready";
module.exports = api;
if (typeof window !== "undefined") {
  window.syntheticApi = module.exports;
}
"#;
    let expected = r#"
const api = () => "ready";
export default api;
if (typeof window !== "undefined") {
  window.syntheticApi = api;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    assert!(validate_output_modules(&[("entry.js".into(), output)])
        .iter()
        .all(|finding| finding.kind != OutputFindingKind::EsmCommonJsResidual));
}

#[test]
fn stable_default_read_recovery_rejects_a_second_assignment() {
    let input = r#"
const api = () => "ready";
module.exports = api;
if (legacy) module.exports = replacement;
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "a conditional second value must keep default reads fail closed:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_rejects_a_reassigned_capture() {
    let input = r#"
let api = () => "ready";
module.exports = api;
api = replacement;
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "a mutable capture does not prove the later CommonJS value:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_rejects_hidden_direct_eval_writes() {
    let input = r#"
let api = () => "ready";
module.exports = api;
eval("api = replacement");
window.syntheticApi = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("window.syntheticApi = module.exports"),
        "direct eval can replace a capture without an AST write site:\n{output}"
    );
}

#[test]
fn stable_default_read_recovery_preserves_direct_calls() {
    let input = r#"
const api = function() { return this.value; };
module.exports = api;
consume(module.exports());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(module.exports())"),
        "a direct CommonJS call supplies module as its receiver:\n{output}"
    );
}

#[test]
fn exports_named_const() {
    let input = "exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn stable_named_function_exports_replace_later_self_reads() {
    let input = r#"
Object.defineProperty(exports, "__esModule", { value: true }),
  exports.second = exports.first = void 0;
var helper = (
  exports.first = function(value) { return value + 1; },
  exports.second = function(value) { return exports.first(value); },
  consume(exports.second(1)),
  function(value) { return value; }
);
"#;
    let expected = r#"
export const first = value => value + 1;
export const second = value => first(value);
consume(second(1));
const helper = value => value;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
    assert!(validate_output_modules(&[("entry.js".into(), output)])
        .iter()
        .all(|finding| finding.kind != OutputFindingKind::EsmCommonJsResidual));
}

#[test]
fn named_export_read_recovery_preserves_direct_eval_receiver_reads() {
    let input = r#"
exports.method = function() { return eval("this.value"); };
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "direct eval can observe the CommonJS receiver:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_hidden_direct_eval_property_writes() {
    let input = r#"
exports.method = () => 1;
eval("exports.method = replacement");
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "direct eval can replace the CommonJS property without an AST write site:\n{output}"
    );
}

#[test]
fn stable_named_object_exports_replace_later_member_reads() {
    let input = r#"
exports.events = createEvents();
exports.notify = () => exports.events.dispatch("ready");
"#;
    let expected = r#"
export const events = createEvents();
export const notify = () => events.dispatch("ready");
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_export_read_recovery_rejects_undefined_after_the_value() {
    let input = r#"
exports.method = () => 1;
exports.method = void 0;
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "a later reset invalidates the recovered property value:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_preserves_receiver_dependent_functions() {
    let input = r#"
exports.method = function() { return this.value; };
consume(exports.method());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method())"),
        "ordinary functions still observe the CommonJS receiver:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_preserves_receiver_dependent_optional_calls_and_tags() {
    let input = r#"
exports.method = function() { return this.value; };
consume(exports.method?.());
consume(exports.method`value`);
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.method?.())")
            && output.contains("consume(exports.method`value`)"),
        "optional calls and tags also supply the CommonJS receiver:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_duplicate_property_writes() {
    let input = r#"
exports.first = () => 1;
exports.first = () => 2;
exports.second = () => exports.first();
"#;
    let output = apply(input);
    assert!(
        output.contains("exports.first()"),
        "a multiply-written property needs value-flow analysis:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_computed_exports_mutation() {
    let input = r#"
exports.first = () => 1;
exports[key] = replacement;
consume(exports.first());
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.first())"),
        "a computed mutation can alias any recovered property:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_rejects_prototype_mutating_member_uses() {
    let getter_installer = r#"
exports.first = () => 1;
exports.__defineGetter__("first", () => () => 2);
consume(exports.first());
"#;
    let output = apply(getter_installer);
    assert!(
        output.contains("consume(exports.first())"),
        "a legacy getter installer can redefine any proven property:\n{output}"
    );

    let prototype_write = r#"
exports.first = () => 1;
exports.__proto__ = fallback;
consume(exports.first());
"#;
    let output = apply(prototype_write);
    assert!(
        output.contains("consume(exports.first())"),
        "a prototype write changes lookup without a visible property write:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_does_not_rewrite_reads_before_the_export() {
    let input = r#"
consume(exports.first);
exports.first = () => 1;
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(exports.first)"),
        "the CommonJS property is not initialized at the earlier read:\n{output}"
    );
}

#[test]
fn named_export_read_recovery_skips_hoisted_function_declarations() {
    let input = r#"
invoke();
exports.first = () => 1;
function invoke() { return exports.first(); }
"#;
    let output = apply(input);
    assert!(
        output.contains("return exports.first()"),
        "the hoisted function may run before the textually earlier-looking export:\n{output}"
    );
}

#[test]
fn local_exports_binding_not_converted_to_export() {
    let input = r#"
var exports = {};
exports.foo = 1;
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn esmodule_marker_on_arbitrary_object_does_not_create_exports_alias() {
    let input = r#"
Object.defineProperty(moduleExports, "__esModule", { value: true });
moduleExports.Service = void 0;
class Service {}
moduleExports.Service = Service;
"#;
    let expected = r#"
Object.defineProperty(moduleExports, "__esModule", {
    value: true
});
moduleExports.Service = undefined;
class Service {}
moduleExports.Service = Service;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_getter_on_exports_to_named_export() {
    let input = r#"
const rawCache = require("./raw-cache.js");
Object.defineProperty(exports, "rawCache", {
  enumerable: true,
  get() {
    return rawCache;
  }
});
"#;
    let expected = r#"
import rawCache from "./raw-cache.js";
export { rawCache };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_member_getter_becomes_live_reexport() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "renamed", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let expected = r#"
export { value as renamed } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_arrow_getter_becomes_live_reexport() {
    let input = r#"
var dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get: () => dep.value
});
"#;
    let expected = r#"
export { value } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_getter_supports_default_reexport() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "default", {
  enumerable: true,
  get: () => dep.value
});
"#;
    let expected = r#"
export { value as default } from "./dep.js";
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn live_reexport_retains_import_when_require_binding_has_other_reads() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
consume(dep.other);
"#;
    let expected = r#"
import dep from "./dep.js";
export { value } from "./dep.js";
consume(dep.other);
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn define_property_member_getter_rejects_reassigned_require_binding() {
    let input = r#"
let dep = require("./dep.js");
dep = replacement;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_member_getter_rejects_member_writes() {
    let input = r#"
const dep = require("./dep.js");
dep.value = replacement;
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_member_getter_rejects_binding_escape() {
    let input = r#"
const dep = require("./dep.js");
consume(dep);
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep.value;
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_member_getter_rejects_dynamic_property() {
    let input = r#"
const dep = require("./dep.js");
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return dep[key];
  }
});
"#;
    let output = apply(input);
    assert!(output.contains("Object.defineProperty(exports, \"value\""));
    assert!(!output.contains("export { value } from"));
}

#[test]
fn define_property_getter_on_arbitrary_object_is_not_export() {
    let input = r#"
Object.defineProperty(moduleExports, "__esModule", {
  value: true
});
Object.defineProperty(moduleExports, "helperValue", {
  enumerable: true,
  get() {
    return helperValue;
  }
});
const helperValue = createHelperValue();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_default_getter_uses_live_export_specifier() {
    let input = r#"
const value = createValue();
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() {
    return value;
  }
});
"#;
    let expected = r#"
const value = createValue();
export { value as default };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn define_property_getter_with_call_return_is_not_export() {
    let input = r#"
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return compute();
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_getter_with_unresolved_return_is_not_export() {
    let input = r#"
Object.defineProperty(exports, "value", {
  enumerable: true,
  get() {
    return globalValue;
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn define_property_getter_with_effectful_descriptor_is_not_export() {
    let input = r#"
const value = createValue();
Object.defineProperty(exports, "value", {
  enumerable: computeEnumerable(),
  get() {
    return value;
  }
});
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn local_module_binding_not_converted_to_export() {
    let input = r#"
var module = { exports: {} };
module.exports = value;
"#;
    let output = render_pipeline_until(input, "UnEsm");
    assert_eq_normalized(&output, input);
}

#[test]
fn exports_named_same_ident() {
    let input = r#"
function foo() {}
exports.foo = foo;
"#;
    let expected = r#"
function foo() {}
export { foo };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop() {
    let input = "exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_keeps_real_default() {
    let input = r#"
exports.default = value;
module.exports = exports.default;
"#;
    let expected = "export default value;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_blocks_unsafe_intervening_call() {
    let input = r#"
exports.default = value;
mutate(exports);
module.exports = exports.default;
"#;
    let expected = r#"
value;
mutate(exports);
export default exports.default;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_blocks_rebinding_exports() {
    let input = r#"
exports.default = value;
exports = other;
module.exports = exports.default;
"#;
    let expected = r#"
value;
exports = other;
export default exports.default;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_allows_safe_intervening_aliases() {
    let input = r#"
exports.default = value;
var imported;
imported = dependency;
var alias = imported;
module.exports = exports.default;
"#;
    let expected = r#"
export default value;
let imported;
imported = dependency;
const alias = imported;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_mirror_keeps_alias_value() {
    let input = r#"
const makeDefault = () => ({});
const entry = makeDefault;
exports.default = entry;
module.exports = exports.default;
"#;
    let expected = r#"
const makeDefault = () => ({});
export default makeDefault;
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_dedup_void_init() {
    // void 0 → undefined after RemoveVoid rule, but the un_esm rule runs and detects void expr
    let input = r#"
exports.foo = void 0;
exports.foo = 1;
"#;
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_dedup_preserves_dropped_rhs_evaluation() {
    let input = r#"
exports.foo = sideEffect1();
exports.foo = sideEffect2();
"#;
    let expected = r#"
sideEffect1();
export const foo = sideEffect2();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn non_top_level_require_unchanged() {
    // VarDeclToLetConst converts var to const since bar is never reassigned.
    let input = r#"
function fn() {
  var bar = require('bar');
}
"#;
    let expected = r#"
function fn() {
  const bar = require('bar');
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_with_prop() {
    let input = "module.exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_named_diff_ident() {
    // UnEsm produces: function bar() {} + export { bar as foo }
    // UnExportRename then renames `bar` → `foo` and promotes to `export function foo() {}`
    let input = r#"
function bar() {}
exports.foo = bar;
"#;
    let expected = r#"export function foo() {}"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop_module_exports() {
    let input = "module.exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn webpack_export_getter_iife_becomes_named_exports() {
    let input = r#"
((exports_1, B)=>{
  for (const G in B) {
    Object.defineProperty(exports_1, G, {
      enumerable: true,
      get: B[G]
    });
  }
})(exports, {
  Foo() { return A; },
  Bar() { return B; }
});
const A = 1;
const B = 2;
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

const DEFAULT_COMPAT_POSTAMBLE: &str = r#"
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;

#[test]
fn named_only_commonjs_surface_drops_dead_default_compat_postamble() {
    let input = format!(
        r#"
const answer = 42;
exports.answer = answer;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    assert_eq_normalized(&output, "export const answer = 42;");
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the recovered named-only module should not retain CommonJS residuals"
    );
}

#[test]
fn logical_expression_default_compat_postamble_is_proven_after_normalization() {
    let input = r#"
const answer = 42;
exports.answer = answer;
("function" == typeof exports.default || "object" == typeof exports.default && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
  value: true
}), Object.assign(exports.default, exports), module.exports = exports.default);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, "export const answer = 42;");
}

#[test]
fn recovered_default_keeps_default_compat_postamble() {
    let input = format!(
        r#"
const answer = 42;
exports.default = answer;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    assert!(output.contains("export default answer"));
    assert!(
        output.contains("Object.assign(exports.default, exports)")
            && output.contains("module.exports = exports.default"),
        "default-object compatibility is not dead when a default exists:\n{output}"
    );
}

#[test]
fn recovered_default_getter_keeps_default_compat_postamble() {
    let input = format!(
        r#"
require.d(exports, {{
  default() {{ return answer; }}
}});
const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );
    let output = apply(&input);
    assert!(
        output.contains("export { answer as default }"),
        "the getter should be recovered as a live default export:\n{output}"
    );
    assert!(
        output.contains("Object.assign(exports.default, exports)")
            && output.contains("module.exports = exports.default"),
        "a recovered ESM default must keep case-2 compatibility intact:\n{output}"
    );
}

#[test]
fn recovered_default_only_getter_rewrites_default_compat_postamble() {
    let input = r#"
Object.defineProperty(exports, "__esModule", {
  value: true
});
Object.defineProperty(exports, "default", {
  enumerable: true,
  get: function() {
    return entry;
  }
});
function entry() {}
("function" == typeof exports.default || "object" == typeof exports.default && null !== exports.default) && void 0 === exports.default.__esModule && (Object.defineProperty(exports.default, "__esModule", {
  value: true
}), Object.assign(exports.default, exports), module.exports = exports.default);
"#;

    let output = apply(input);
    assert_eq_normalized(
        &output,
        r#"
export { entry as default };
function entry() {}
if ((typeof entry === "function" || typeof entry === "object" && entry !== null) && entry.__esModule === undefined) {
    Object.defineProperty(entry, "__esModule", {
        value: true
    });
    entry.default = entry;
}
"#,
    );
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the rewritten default-only adapter should leave no CommonJS residual"
    );
}

#[test]
fn shadowed_exports_and_module_do_not_block_default_only_postamble_recovery() {
    let input = format!(
        r#"
Object.defineProperty(exports, "__esModule", {{
  value: true
}});
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{
    return entry;
  }}
}});
function entry(exports, module) {{
  return exports ?? module;
}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
    );

    let output = apply(&input);
    assert!(!output.contains("Object.assign(exports.default, exports)"));
    assert!(!output.contains("module.exports = exports.default"));
    assert!(output.contains("export { entry as default }"));
    assert!(output.contains("entry.default = entry"));
}

#[test]
fn default_only_type_helper_is_preserved_on_the_recovered_binding() {
    let input = r#"
const typeOf = require("./typeof.js");
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() {
    return entry;
  }
});
function entry() {}
if ((typeof exports.default === "function" || typeOf(exports.default) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;

    let output = apply(input);
    assert!(output.contains("typeOf(entry) === \"object\""), "{output}");
    assert!(output.contains("entry.default = entry"), "{output}");
    assert!(!output.contains("exports.default"), "{output}");
    assert!(!output.contains("module.exports"), "{output}");
}

#[test]
fn non_exact_default_surfaces_keep_default_compat_postamble() {
    let cases = [
        (
            "named export getter",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
Object.defineProperty(exports, "answer", {{
  enumerable: true,
  get() {{ return answer; }}
}});
function entry() {{}}
const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "exports alias",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
const publicApi = exports;
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "other module use",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
inspect(module);
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "direct eval",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{ eval(source); }}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "default assignment instead of generated getter",
            format!("function entry() {{}}\nexports.default = entry;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "authored ESM default",
            format!(
                "function entry() {{}}\nexport {{ entry as default }};\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "mixed authored ESM surface",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
export const answer = 42;
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "getter re-export",
            format!(
                r#"
const dependency = require("./dependency.js");
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return dependency.default; }}
}});
                {DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "shadowed Object helper",
            format!(
                r#"
const Object = customObject;
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "duplicate default getter",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return replacement; }}
}});
function entry() {{}}
function replacement() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "spread default getter argument",
            format!(
                r#"
Object.defineProperty(...exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
"#
            ),
        ),
        (
            "postamble is not final",
            format!(
                r#"
Object.defineProperty(exports, "default", {{
  enumerable: true,
  get() {{ return entry; }}
}});
function entry() {{}}
{DEFAULT_COMPAT_POSTAMBLE}
observe(entry);
"#
            ),
        ),
        (
            "postamble has an alternate branch",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
} else {
  observe(exports.default);
}
"#
            .to_string(),
        ),
        (
            "effectful compatibility descriptor",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true,
    configurable: touch(exports)
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
        (
            "multi-argument type helper",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || typeOf(exports.default, exports) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
        (
            "CommonJS type helper receiver",
            r#"
Object.defineProperty(exports, "default", {
  enumerable: true,
  get() { return entry; }
});
function entry() {}
if ((typeof exports.default === "function" || exports.typeOf(exports.default) === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#
            .to_string(),
        ),
    ];

    for (name, input) in cases {
        let output = apply(&input);
        assert!(
            output.contains("Object.assign(exports.default, exports)")
                && output.contains("module.exports = exports.default"),
            "{name} must fail closed:\n{output}"
        );
    }
}

#[test]
fn dynamic_commonjs_surfaces_keep_default_compat_postamble() {
    let cases = [
        (
            "computed write",
            format!("const name = chooseName();\nexports[name] = 1;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "exports alias",
            format!("const publicApi = exports;\npublicApi.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "hidden default descriptor",
            format!(
                "Object.defineProperty(exports, \"default\", {{ value: 42 }});\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "dynamic getter helper",
            format!(
                "const name = chooseName();\nrequire.d(exports, name, () => 42);\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "direct eval",
            format!("exports.answer = 42;\neval(source);\n{DEFAULT_COMPAT_POSTAMBLE}"),
        ),
        (
            "hoisted default writer",
            format!(
                "exports.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}\nfunction installDefault() {{ exports.default = 42; }}"
            ),
        ),
        (
            "prototype reassignment",
            format!(
                "exports.__proto__ = {{ default: {{}} }};\nexports.answer = 42;\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
        (
            "legacy default getter installer",
            format!(
                "exports.answer = 42;\nexports.__defineGetter__(\"default\", () => ({{ answer: exports.answer }}));\n{DEFAULT_COMPAT_POSTAMBLE}"
            ),
        ),
    ];

    for (name, input) in cases {
        let output = apply(&input);
        assert!(
            output.contains("Object.assign(exports.default, exports)")
                && output.contains("module.exports = exports.default"),
            "{name} must fail closed:\n{output}"
        );
    }
}

#[test]
fn prototype_mutating_exports_write_stays_commonjs_residual() {
    let output = apply("exports.__proto__ = { legacy: true };\nexports.answer = 42;\n");
    assert!(
        !output.contains("export const __proto__"),
        "a prototype write is not a named export:\n{output}"
    );
    assert!(
        output.contains("exports.__proto__"),
        "the prototype write must stay visible as a residual:\n{output}"
    );
    assert!(
        output.contains("export const answer = 42"),
        "ordinary named exports still convert around the residual:\n{output}"
    );
}

#[test]
fn export_getter_map_with_prototype_mutating_name_stays_residual() {
    let input = r#"
require.d(exports, {
  __proto__() { return legacy; },
  real() { return value; }
});
const value = 1;
"#;
    let output = apply(input);
    assert!(
        output.contains("require.d(exports"),
        "converting would synthesize a prototype write for the __proto__ entry:\n{output}"
    );
    assert!(
        !output.contains("export "),
        "no partial conversion of the remaining map entries:\n{output}"
    );
}

#[test]
fn single_export_getter_with_prototype_mutating_name_stays_residual() {
    let output = apply("require.d(exports, \"__proto__\", () => 42);\n");
    assert!(
        output.contains("require.d(exports"),
        "the original own accessor definition must stay as a residual:\n{output}"
    );
}

#[test]
fn webpack_export_getter_iife_recovers_live_default_after_declaration() {
    let input = r#"
((target, getters) => {
  for (const key in getters) {
    Object.defineProperty(target, key, {
      enumerable: true,
      get: getters[key]
    });
  }
})(exports, {
  dim() { return dim; },
  default() { return logger; }
});
function dim(value) {
  return value;
}
const logger = {
  warn(value) { console.warn(value); }
};
"#;
    let output = apply(input);
    assert!(
        !output.contains("Object.defineProperty") && !output.contains("getters"),
        "the recognized getter-map helper should be removed:\n{output}"
    );
    assert!(
        output.contains("export { dim };") || output.contains("export function dim"),
        "the named getter should remain a live named export:\n{output}"
    );
    assert!(
        output.contains("export { logger as default };"),
        "the default getter should remain a live default export:\n{output}"
    );
    let declaration = output
        .find("const logger")
        .expect("logger declaration should remain");
    let export = output
        .find("export { logger as default };")
        .expect("default export should be recovered");
    assert!(
        declaration < export,
        "the default export must be deferred past its declaration:\n{output}"
    );
}

#[test]
fn webpack_getter_default_deferred_to_end() {
    // Webpack5 export getters place the getter map at the top of the module,
    // before declarations.  Named exports are fine (live bindings), but
    // `default` exports evaluate eagerly.  The default entry must be deferred
    // to the end of the module body to avoid TDZ violations.
    let input = r#"
require.d(exports, {
  default() { return o; },
  VERSION() { return VERSION; }
});
const r = { apiBase: "https://example.com" };
const o = r;
const VERSION = "2.1.0";
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn direct_webpack_export_getters_become_named_exports() {
    let input = r#"
require.d(exports, "APP_NAME", ()=>n);
require.d(exports, "readSetting", ()=>i);
const n = "Revenue Console";
function i(t, e = null) {
  return e;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn direct_webpack_export_getter_member_return_does_not_leak_helper() {
    let input = r#"
const effects = require("./effects.js");
require.d(exports, "take", ()=>effects.take);
"#;
    let output = apply(input);
    assert!(
        !output.contains("require.d"),
        "webpack export getter helper should not survive:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn direct_webpack_export_getter_map_becomes_named_exports() {
    let input = r#"
require.d(exports, {
  APP_NAME() { return n; },
  readSetting() { return i; }
});
const n = "Revenue Console";
function i(t, e = null) {
  return e;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn unused_iife_with_webpack_export_getters_becomes_module_exports() {
    let input = r#"
"use strict";
((t)=>{
  require.d(exports, "VERSION", ()=>o);
  require.d(exports, "getConfig", ()=>i);
  require.d(exports, "mergeConfig", ()=>u);
  const r = {
    apiBase: "https://example.com",
    timeout: 5000
  };
  exports.default = r;
  const o = "2.1.0";
  function i(t) {
    return r[t];
  }
  function u(t) {
    return { ...r, ...t };
  }
})(require("./module-11.js"));
"#;
    let output = apply(input);
    assert!(
        !output.contains("require.d"),
        "webpack export getter helper should not survive:\n{output}"
    );
    assert!(
        output.contains("\"use strict\""),
        "the leading strict directive must survive IIFE exposure:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn iife_with_used_param_keeps_webpack_export_getter_wrapped() {
    let input = r#"
((t)=>{
  require.d(exports, "value", ()=>t.value);
})(require("./dep.js"));
"#;
    let output = apply(input);
    assert!(
        output.contains("require.d"),
        "webpack export getter should stay wrapped when the IIFE param is used:\n{output}"
    );
    insta::assert_snapshot!(output);
}

#[test]
fn webpack_export_getter_iife_keeps_non_compat_if_block() {
    let input = r#"
((exports_1, B)=>{
  for (const G in B) {
    Object.defineProperty(exports_1, G, {
      enumerable: true,
      get: B[G]
    });
  }
})(exports, {
  Foo() { return A; }
});
const A = 1;
if (flag) {
  Object.defineProperty(exports.default, "__esModule", {
    value: true
  });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn void_only_export_removed() {
    let input = "exports.foo = void 0;";
    let expected = "";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn self_ref_pattern_removed() {
    let input = "module.exports.default = module.exports;";
    let expected = "";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn coupled_lazy_default_helper_uses_live_binding_and_preserves_self_mirrors() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => typeof next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    let expected = r#"
function helper(value) {
  helper = next => typeof next;
  helper.default = helper;
  return helper(value);
}
export { helper as default };
helper.default = helper;
"#;
    assert_eq_normalized(&output, expected);
    assert!(
        validate_output_modules(&[("entry.js".into(), output)]).is_empty(),
        "the recovered live default should be a valid ESM module"
    );
}

#[test]
fn coupled_lazy_default_helper_ignores_shadowed_module_bindings() {
    let input = r#"
function helper(value) {
  {
    const module = { exports: "local" };
    consume(module.exports);
  }
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(module.exports)"),
        "a lexically shadowed module binding must remain untouched:\n{output}"
    );
    assert_eq!(
        output.matches("module.exports").count(),
        1,
        "only the shadowed local read should remain:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_handles_mutually_exclusive_replacements() {
    let input = r#"
function helper(value) {
  if (supportsFastPath) {
    module.exports = helper = fastPath;
  } else {
    module.exports = helper = slowPath;
  }
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        !output.contains("module.exports"),
        "every whole-value replacement is coupled to the same helper binding:\n{output}"
    );
    assert!(
        output.contains("helper = fastPath") && output.contains("helper = slowPath"),
        "both runtime branches must keep their original binding updates:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_handles_called_sequence_value() {
    let input = r#"
function helper() {
  return (module.exports = helper = () => true,
    module.exports.__esModule = true,
    module.exports.default = module.exports)();
}
module.exports = helper;
module.exports.__esModule = true;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        !output.contains("module.exports"),
        "the called sequence should use the proven coupled binding:\n{output}"
    );
    assert!(
        output.contains("helper.__esModule = true"),
        "a nested marker that is part of the runtime value must stay observable:\n{output}"
    );
    assert!(output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_uncoupled_module_replacement() {
    let input = r#"
function helper(value) {
  module.exports = chooseOtherValue();
  module.exports.default = module.exports;
  return value;
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports = chooseOtherValue()")
            && output.contains("module.exports.default = module.exports"),
        "an uncoupled CommonJS replacement must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "the ordinary snapshot default must not be upgraded without coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_independent_binding_write() {
    let input = r#"
function helper(value) {
  helper = chooseOtherValue();
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "the helper and CommonJS value can diverge before the coupled write:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_direct_eval() {
    let input = r#"
function helper(value) {
  eval(source);
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "direct eval can observe or mutate both candidate identities:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_receiver_sensitive_call() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return module.exports(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports(value)"),
        "a direct member call supplies the CommonJS module as `this`:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_exports_alias_use() {
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  exports.alias = helper;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports"),
        "exports still names the initial object after module.exports is replaced:\n{output}"
    );
    assert!(!output.contains("export { helper as default }"), "{output}");
}

#[test]
fn existing_import_absorbed() {
    let input = r#"
import { a } from 'foo';
var { b } = require('foo');
"#;
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn compound_assign_not_transformed() {
    // module.exports += 1 should NOT be transformed
    let input = "module.exports += 1;";
    let expected = "module.exports += 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bracket_notation_module_exports_transformed() {
    // module["exports"] is normalized to module.exports by UnBracketNotation,
    // then converted to ESM by UnEsm
    let input = r#"module["exports"] = 1;"#;
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_name_takes_priority_over_conflicting_local() {
    // When exports.a = expr and `a` is already a local binding,
    // the local should be renamed so the export keeps the clean name.
    let input = r#"
var a = 0;
exports.a = function(x) { return a + x; };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn export_conflict_rename_avoids_nested_shadow_capture() {
    let input = r#"
var a = 0;
function f(_a) { return a + _a; }
exports.a = function(x) { return a + f(x); };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn export_conflict_rename_preserves_object_pattern_key() {
    let input = r#"
var obj = { a: 1 };
var { a } = obj;
exports.a = function(x) { return a + x; };
"#;
    let output = render_pipeline_until(input, "UnEsm");
    // Destructuring must produce `{ a: _a }`, not `{ _a }` — the property key stays `a`.
    insta::assert_snapshot!(output);
}

#[test]
fn no_rename_when_export_name_is_free() {
    // No conflict — export name is not used by any local binding
    let input = r#"
var b = 0;
exports.a = function(x) { return b + x; };
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn reserved_named_export_uses_safe_local_binding() {
    let input = r#"
exports.eval = function(source) {
    return eval(source);
};
exports.in = 1;
"#;
    let expected = r#"
var _eval = function(source) {
    return eval(source);
};
export { _eval as eval };
var _in = 1;
export { _in as in };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn named_export_does_not_capture_existing_global_reference() {
    let input = r#"
var marker = typeof runtime !== "undefined" && runtime.pid ? runtime.pid : "";
module.exports = module.exports.default = function() {
    return marker;
};
module.exports.runtime = function() {
    return marker;
};
"#;
    let expected = r#"
const marker = typeof runtime !== "undefined" && runtime.pid ? runtime.pid : "";
export default module.exports.default = () => marker;
const _runtime = () => marker;
export { _runtime as runtime };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn compound_exports_assignment_in_var_decl() {
    // var s = exports.history = expr → split into var s = expr + export { s as history }
    let input = r#"
var s = exports.history = createBrowserHistory();
use(s);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn compound_exports_same_name_merges_to_export_decl() {
    // var SessionContext = exports.SessionContext = expr
    // → export var SessionContext = expr (merge preserves original decl kind)
    let input = r#"
"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.SessionContext = void 0;
var SessionContext = exports.SessionContext = React.createContext(undefined);
use(SessionContext);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

// ============================================================
// Require hoisting from complex expressions
// ============================================================

#[test]
fn hoist_require_from_seq_expr_in_export_default() {
    let input = r#"
let i;
export default (i = require("./a.js"), require("./b.js"), i.foo);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn hoist_require_call_invocation() {
    let input = r#"
export default require("./factory.js")();
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn inline_conditional_interop_to_import() {
    let input = r#"
let i;
const a = (i = require("./react.js")) && i.__esModule ? i : { default: i };
console.log(a);
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn inline_conditional_interop_default_only_to_default_import() {
    let input = r#"
let n;
const r = (n = require("./base.js")) && n.__esModule ? n : { default: n };
function build() {
  return factory(r.default);
}
"#;
    let expected = r#"
import r from "./base.js";
let n;
function build() {
  return factory(r);
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);

    let expected_final = r#"
import r from "./base.js";
function build() {
  return factory(r);
}
"#;
    assert_eq_normalized(&render_pipeline(input), expected_final);
}

#[test]
fn inline_conditional_interop_default_recovery_is_binding_aware() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
function read(r) {
  return r.default;
}
consume(r.default, read(other));
"#;
    let expected = r#"
import r from "./dep.js";
let n;
function read(r) {
  return r.default;
}
consume(r, read(other));
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_conditional_interop_default_recovery_handles_optional_member_read() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r?.default);
"#;
    let output = apply(input);
    assert!(
        !output.contains(".default") || !output.contains("import r from"),
        "default-only recovery must rewrite every accepted access: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_mixed_wrapper_uses() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r.default, r);
"#;
    let output = apply(input);
    assert!(
        output.contains("consume(r.default, r)"),
        "a wrapper that escapes must keep its Babel interop semantics: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_writes() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
r.default = replacement;
"#;
    let output = apply(input);
    assert!(
        output.contains(".default = replacement") && !output.contains("import r from"),
        "a written wrapper property must not use the default-only recovery: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_dynamic_properties() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r[key]);
"#;
    let output = apply(input);
    assert!(
        output.contains("[key]") && !output.contains("import r from"),
        "a dynamically accessed wrapper must not use the default-only recovery: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_rejects_used_require_temp() {
    let input = r#"
let n;
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
consume(r.default, n);
"#;
    let output = apply(input);
    assert!(
        output.contains("n = _n")
            && output.contains("consume(n.default, n)")
            && !output.contains("import r from"),
        "a require temp used outside the helper must keep its assignment: {output}"
    );
}

#[test]
fn inline_conditional_interop_default_recovery_preserves_late_let_tdz() {
    let input = r#"
const r = (n = require("./dep.js")) && n.__esModule ? n : { default: n };
let n;
consume(r.default);
"#;
    let expected = r#"
import _n from "./dep.js";
n = _n;
const r = n;
let n;
consume(r.default);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn inline_conditional_interop_rejects_mismatched_shape() {
    let input = r#"
let i;
let j;
const a = (i = require("./react.js")) && j.__esModule ? i : { default: j };
"#;
    let output = apply(input);
    assert!(
        output.contains("require(\"./react.js\")") && output.contains("j.__esModule"),
        "mismatched inline conditional should not be hoisted as Babel interop: {output}"
    );
}

#[test]
fn plain_export_default_require_not_hoisted() {
    // export default require("...") should NOT be hoisted — it's a valid re-export
    // that namespace_decomposition can see through.
    let input = r#"
export default require("./module.js");
"#;
    let output = apply(input);
    insta::assert_snapshot!(output);
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_compound_module_write() {
    // `*=` reads module.exports before writing; deleting the read-modify-
    // write is not a coupled replacement.
    let input = r#"
function helper(value) {
  module.exports *= helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports *="),
        "a compound CommonJS write must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "compound writes must fail the coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_logical_module_write() {
    // `||=` only evaluates its right side when module.exports is falsy;
    // the original never reassigns the helper here.
    let input = r#"
function helper(value) {
  module.exports ||= helper = (next) => next;
  module.exports.default = module.exports;
  return helper(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports ||="),
        "a conditional CommonJS write must remain visible:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "logical assignment must fail the coupling proof:\n{output}"
    );
}

#[test]
fn coupled_lazy_default_helper_fails_closed_on_optional_chained_module_use() {
    // The rewriter substitutes only plain `module.exports` members; an
    // optional-chained access would survive as an orphaned free `module`.
    let input = r#"
function helper(value) {
  module.exports = helper = (next) => next;
  module.exports.default = module.exports;
  return module?.exports(value);
}
module.exports = helper;
module.exports.default = module.exports;
"#;
    let output = apply(input);
    assert!(
        output.contains("module.exports = helper"),
        "the coupled write must stay when an optional-chained use exists:\n{output}"
    );
    assert!(
        !output.contains("export { helper as default }"),
        "optional-chained module access must fail the coupling proof:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Top-level Call args: require("mod").Name → named import
//
// Producer: a CJS compiler emits a static `require(mod).Name` as a direct
// argument of an immediately-evaluated top-level call (typically an IIFE).
// UnEsm already converts `var x = require("mod").Name` via NamedProp; this
// shape never reached that classifier. `.default` args use a parallel pass.
// ---------------------------------------------------------------------------

fn apply_unesm(input: &str) -> String {
    render_pipeline_until(input, "UnEsm")
}

#[test]
fn toplevel_iife_require_named_member_arg_to_named_import() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
    assert!(
        !output.contains("require("),
        "the hoisted named member must become an import:\n{output}"
    );
}

#[test]
fn toplevel_iife_require_named_member_arg_survives_later_rules() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply(input);
    assert!(
        output.contains("import { UIBase } from \"./UIBase.js\"") && !output.contains("require("),
        "later rules must keep the named import:\n{output}"
    );
}

#[test]
fn toplevel_var_init_iife_require_named_member_arg_to_named_import() {
    let input = r#"
var Child = (function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
var Child = function (base) {
  use(base);
}(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_computed_ident_require_named_member_arg_to_named_import() {
    // UnBracketNotation may already fold this to `.UIBase`; UnEsm must still
    // accept `is_ident_prop` computed strings if the member survives.
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js")["UIBase"]);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_arg_reuses_existing_named_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_arg_reuses_existing_named_prop_through_full_pipeline() {
    // Replacing the argument with make_ident() would drop the resolved ctxt of
    // `var UIBase`, so DeadImports would treat the named import as unused.
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_require_named_member_arg_keeps_import_through_full_pipeline() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_does_not_reuse_later_named_prop() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
var UIBase = require("./UIBase.js").UIBase;
var keep = require("./keep.js");
"#;
    let expected = r#"
import { UIBase } from "./UIBase.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_does_not_reuse_mutated_named_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").UIBase;
UIBase = other;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
var keep = require("./keep.js");
"#;
    let output = apply_unesm(input);
    assert!(
        output.contains("require(\"./UIBase.js\").UIBase")
            && output.contains("import keep from \"./keep.js\""),
        "a mutated NamedProp local must not be reused as the call argument:\n{output}"
    );
}

#[test]
fn toplevel_iife_require_default_member_arg_to_default_import() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
    assert!(
        !output.contains("require("),
        "the hoisted default member must become an import:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_uses_readable_fallback_for_numeric_module_name() {
    let input = r#"
(function (base) {
  use(base);
})(require("./module-42.js").default);
consume(require("./module-43.js").default);
consume(require("./module-44.js").default);
"#;
    let expected = r#"
import defaultExport from "./module-42.js";
import defaultExport_1 from "./module-43.js";
import defaultExport_2 from "./module-44.js";
(function (base) {
  use(base);
})(defaultExport);
consume(defaultExport_1);
consume(defaultExport_2);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_avoids_readable_fallback_collision() {
    let input = r#"
let defaultExport = existing;
consume(require("./module-42.js").default);
"#;
    let expected = r#"
import defaultExport_1 from "./module-42.js";
let defaultExport = existing;
consume(defaultExport_1);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_ternary_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(cond ? require("./UIBase.js").UIBase : other);
"#;
    let expected = r#"
import keep from "./keep.js";
f(cond ? require("./UIBase.js").UIBase : other);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_inside_then_callback_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
then(function () {
  return require("./UIBase.js").UIBase;
});
"#;
    let expected = r#"
import keep from "./keep.js";
then(function () {
  return require("./UIBase.js").UIBase;
});
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_inside_function_body_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").UIBase);
}
"#;
    let expected = r#"
import keep from "./keep.js";
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").UIBase);
}
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_dynamic_require_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require(dyn).UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require(dyn).UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_spread_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(...require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
f(...require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_computed_dynamic_key_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js")[key]);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js")[key]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_local_require_binding_is_left_alone() {
    let input = r#"
function require(x) {
  return x;
}
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_named_member_comma_expr_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})((0, require("./UIBase.js").UIBase));
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})((0, require("./UIBase.js").UIBase));
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_existing_import_local() {
    let input = r#"
import { UIBase } from "./other.js";
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_existing_let_binding() {
    let input = r#"
let UIBase = 0;
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
let UIBase = 0;
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_reference() {
    let input = r#"
observe(UIBase);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
observe(UIBase);
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_assignment_target() {
    let input = r#"
UIBase = globalValue;
var keep = require("./keep.js");
consume(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
UIBase = globalValue;
consume(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_unresolved_jsx_tag() {
    let input = r#"
render(<UIBase />);
var keep = require("./keep.js");
consume(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
render(<UIBase />);
consume(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_two_sources_one_local() {
    let input = r#"
var keep = require("./keep.js");
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").A);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").A);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_eval_of_name() {
    let input = r#"
eval("UIBase");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
eval("UIBase");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_dynamic_eval() {
    let input = r#"
eval(source);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let expected = r#"
import keep from "./keep.js";
eval(source);
(function (base) {
  use(base);
})(require("./UIBase.js").UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_reserved_prop() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./X.js").class);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./X.js").class);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_illegal_ident_prop() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./X.js")["foo-bar"]);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require("./X.js")["foo-bar"]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_invalid_unicode_ident_prop() {
    let input = r#"
var keep = require("./keep.js");
f(require("./X.js")["a²"]);
"#;
    let expected = r#"
import keep from "./keep.js";
f(require("./X.js")["a²"]);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_early_self_read_keeps_commonjs_boundary() {
    let input = r#"
consume(require("./module-1.js").value);
exports.value = 1;
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a direct named self-read must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn toplevel_require_named_member_fails_closed_on_provider_member_write() {
    let input = r#"
consume(require("./dep.js").UIBase);
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_named_member_fails_closed_on_other_provider_member_mutations() {
    let mutations = [
        r#"require("./dep.js").UIBase += replacement;"#,
        r#"require("./dep.js").UIBase++;"#,
        r#"delete require("./dep.js").UIBase;"#,
        r#"for (require("./dep.js").UIBase in values) {}"#,
        r#"for (require("./dep.js").UIBase of values) {}"#,
    ];

    for mutation in mutations {
        let input = format!(
            r#"
consume(require("./dep.js").UIBase);
{mutation}
consume(require("./dep.js").UIBase);
"#
        );
        let output = apply_unesm(&input);

        assert!(
            !output.contains("import { UIBase }")
                && output.matches("require(\"./dep.js\").UIBase").count() >= 2,
            "provider mutation must keep fresh member reads:\n{output}"
        );
    }
}

#[test]
fn toplevel_require_named_member_does_not_reuse_local_across_provider_member_write() {
    let input = r#"
var UIBase = require("./dep.js").UIBase;
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let expected = r#"
import { UIBase } from "./dep.js";
require("./dep.js").UIBase = replacement;
consume(require("./dep.js").UIBase);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_sibling_named_import_does_not_block_recovery() {
    let input = r#"
import { keep } from "./keep.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { keep } from "./keep.js";
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_var_init_iife_require_default_member_arg_to_default_import() {
    let input = r#"
var Child = (function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
var Child = function (base) {
  use(base);
}(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_computed_require_default_member_arg_to_default_import() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js")["default"]);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_import() {
    let input = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_promotes_type_only_import_for_runtime_value() {
    let input = r#"
import type UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = wakaru_core::decompile(
        input,
        wakaru_core::DecompileOptions {
            filename: "fixture.ts".to_string(),
            ..Default::default()
        },
    )
    .expect("TypeScript input should decompile")
    .code;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_import_through_full_pipeline() {
    let input = r#"
import UIBase from "./UIBase.js";
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_arg_reuses_existing_default_prop_through_full_pipeline() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_iife_require_default_member_arg_keeps_import_through_full_pipeline() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
((base) => {
  use(base);
})(UIBase);
"#;
    let output = render_pipeline(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_shares_local_across_same_source_args() {
    let input = r#"
(function (first) {
  use(first);
})(require("./UIBase.js").default);
(function (second) {
  use(second);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import UIBase from "./UIBase.js";
(function (first) {
  use(first);
})(UIBase);
(function (second) {
  use(second);
})(UIBase);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_matches_import() {
    let input = r#"
import { UIBase } from "./other.js";
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_matches_let() {
    let input = r#"
let UIBase = 0;
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
let UIBase = 0;
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_basename_is_unresolved() {
    let input = r#"
observe(UIBase);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
observe(UIBase);
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_for_unresolved_assignment_target() {
    let input = r#"
UIBase = globalValue;
consume(require("./UIBase.js").default);
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
UIBase = globalValue;
consume(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_for_named_default_declaration() {
    let input = r#"
export default function UIBase() {}
consume(require("./UIBase.js").default);
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
export default function UIBase() {}
consume(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_ternary_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(cond ? require("./UIBase.js").default : other);
"#;
    let expected = r#"
import keep from "./keep.js";
f(cond ? require("./UIBase.js").default : other);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_inside_then_callback_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
then(function () {
  return require("./UIBase.js").default;
});
"#;
    let expected = r#"
import keep from "./keep.js";
then(function () {
  return require("./UIBase.js").default;
});
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_inside_function_body_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").default);
}
"#;
    let expected = r#"
import keep from "./keep.js";
function wrap() {
  (function (base) {
    use(base);
  })(require("./UIBase.js").default);
}
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_dynamic_require_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require(dyn).default);
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})(require(dyn).default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_spread_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
f(...require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
f(...require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_local_require_binding_is_left_alone() {
    let input = r#"
function require(x) {
  return x;
}
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_default_member_comma_expr_arg_is_left_alone() {
    let input = r#"
var keep = require("./keep.js");
(function (base) {
  use(base);
})((0, require("./UIBase.js").default));
"#;
    let expected = r#"
import keep from "./keep.js";
(function (base) {
  use(base);
})((0, require("./UIBase.js").default));
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_unknown_eval() {
    let input = r#"
eval(source);
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
eval(source);
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_falls_back_when_eval_mentions_basename() {
    let input = r#"
eval("UIBase");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import defaultExport from "./UIBase.js";
eval("UIBase");
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_eval_of_synthetic_name() {
    let input = r#"
import { UIBase } from "./other.js";
eval("defaultExport");
var keep = require("./keep.js");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let expected = r#"
import { UIBase } from "./other.js";
import keep from "./keep.js";
eval("defaultExport");
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_early_self_read_keeps_commonjs_boundary() {
    let input = r#"
consume(require("./module-1.js").default);
exports.default = 1;
"#;
    let output = render_pipeline_until_with_filename(input, "UnEsm", "module-1.js");

    assert_eq_normalized(&output, input);
    assert!(
        !output.contains("import ") && !output.contains("export "),
        "a direct default self-read must not cross only part of its CommonJS boundary:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_fails_closed_on_provider_member_write() {
    let input = r#"
consume(require("./dep.js").default);
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, input);
}

#[test]
fn toplevel_require_default_member_fails_closed_on_other_provider_member_mutations() {
    let mutations = [
        r#"require("./dep.js").default += replacement;"#,
        r#"require("./dep.js").default++;"#,
        r#"delete require("./dep.js").default;"#,
        r#"for (require("./dep.js").default in values) {}"#,
        r#"for (require("./dep.js").default of values) {}"#,
    ];

    for mutation in mutations {
        let input = format!(
            r#"
consume(require("./dep.js").default);
{mutation}
consume(require("./dep.js").default);
"#
        );
        let output = apply_unesm(&input);

        assert!(
            !output.contains("import ")
                && output.matches("require(\"./dep.js\").default").count() >= 2,
            "provider mutation must keep fresh default member reads:\n{output}"
        );
    }
}

#[test]
fn toplevel_require_default_member_does_not_reuse_later_default_prop() {
    let input = r#"
(function (base) {
  use(base);
})(require("./UIBase.js").default);
var UIBase = require("./UIBase.js").default;
var keep = require("./keep.js");
"#;
    let expected = r#"
import defaultExport from "./UIBase.js";
import UIBase from "./UIBase.js";
import keep from "./keep.js";
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_does_not_reuse_mutated_default_prop() {
    let input = r#"
var UIBase = require("./UIBase.js").default;
UIBase = other;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
var keep = require("./keep.js");
"#;
    let expected = r#"
import _UIBase from "./UIBase.js";
import defaultExport from "./UIBase.js";
import keep from "./keep.js";
var UIBase = _UIBase;
UIBase = other;
(function (base) {
  use(base);
})(defaultExport);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_skips_written_binding_and_reuses_later_stable() {
    // Basename UIBase is taken and eval mentions `defaultExport`, so recovery is
    // only possible by reusing the later unwritten DefaultProp.
    let input = r#"
import { UIBase } from "./other.js";
eval("defaultExport");
var keep = require("./keep.js");
var poisoned = require("./UIBase.js").default;
poisoned = other;
var helper = require("./UIBase.js").default;
(function (base) {
  use(base);
})(require("./UIBase.js").default);
"#;
    let output = apply_unesm(input);
    assert!(
        output.contains("import keep from \"./keep.js\"")
            && output.contains("import helper from \"./UIBase.js\"")
            && output.contains("})(helper)")
            && !output.contains("require(\"./UIBase.js\").default"),
        "a later unwritten DefaultProp must still be reusable after a mutated one:\n{output}"
    );
}

#[test]
fn toplevel_require_default_member_does_not_reuse_local_across_provider_member_write() {
    let input = r#"
var UIBase = require("./dep.js").default;
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let expected = r#"
import UIBase from "./dep.js";
require("./dep.js").default = replacement;
consume(require("./dep.js").default);
"#;
    let output = apply_unesm(input);

    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_named_member_failure_does_not_block_default_member() {
    let input = r#"
import { A } from "./other.js";
var keep = require("./keep.js");
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let expected = r#"
import { A } from "./other.js";
import keep from "./keep.js";
import B from "./B.js";
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(B);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn toplevel_require_default_member_failure_does_not_block_named_member() {
    let input = r#"
var keep = require("./keep.js");
require("./B.js").default = replacement;
(function (a) {
  use(a);
})(require("./A.js").A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let expected = r#"
import keep from "./keep.js";
import { A } from "./A.js";
require("./B.js").default = replacement;
(function (a) {
  use(a);
})(A);
(function (b) {
  use(b);
})(require("./B.js").default);
"#;
    let output = apply_unesm(input);
    assert_eq_normalized(&output, expected);
}
