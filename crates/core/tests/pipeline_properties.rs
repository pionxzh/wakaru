//! End-to-end pipeline property tests.
//!
//! Per-rule tests and snapshots verify each rule in isolation; these tests
//! assert invariants over the FULL pipeline's output, because rules can
//! individually pass while destroying each other's work on realistic module
//! shapes (2026-08-05: UnJsx capitalized a component import correctly, then
//! UnImportRename de-aliased it back into a lowercase JSX tag; the export
//! audit blocked alias-safe renames every rule test said were fine).
//!
//! The properties, not specific outputs, are the contract:
//! - a module's public export names survive the pipeline unchanged;
//! - JSX element names never bind to lowercase component bindings;
//! - a decompiled module pair forms a valid graph (no dangling refs, no
//!   missing imported names, no writes to imports or consts).

mod common;

use common::render;
use wakaru_core::{decompile, validate_output_modules, DecompileOptions, UnpackWarningKind};

/// Decompile each module independently (as real single-file jobs do), then
/// validate the results as one graph. Consumer imports pin the providers'
/// public contract: any public-name drift or introduced write surfaces as a
/// validator finding.
fn assert_pipeline_pair_valid(pairs: &[(&str, &str)]) {
    let outputs: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, source)| (name.to_string(), render(source)))
        .collect();
    let findings = validate_output_modules(&outputs);
    assert!(
        findings.is_empty(),
        "pipeline output violates graph invariants: {findings:#?}\n--- outputs ---\n{}",
        outputs
            .iter()
            .map(|(name, code)| format!("// {name}\n{code}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn public_export_contract_survives_rename_bait() {
    // The provider carries every export shape next to rename bait
    // (hook init, displayName, minified names). Whatever the pipeline
    // renames locally, the consumer's imports must still resolve.
    let provider = r#"
import { jsx } from "react/jsx-runtime";
const a = useRef();
const b = ({ children })=>jsx("div", { children });
b.displayName = "FancyCard";
const c = 1;
export { a, b as Z, c };
export const keep = a;
export default function entry() {
    return jsx(b, { children: c });
}
"#;
    let consumer = r#"
import entry, { a, Z, c, keep } from "./provider.js";
console.log(entry, a, Z, c, keep);
"#;
    assert_pipeline_pair_valid(&[("provider.js", provider), ("consumer.js", consumer)]);
}

#[test]
fn exported_builtin_alias_keeps_consumer_edge_valid() {
    let provider = r#"
var defineProperty = Object.defineProperty;
export { defineProperty };
"#;
    let consumer = r#"
import { defineProperty } from "./provider.js";
export const define = defineProperty;
"#;
    assert_pipeline_pair_valid(&[("provider.js", provider), ("consumer.js", consumer)]);
}

#[test]
fn jsx_component_tags_never_bind_lowercase() {
    // Lowercase component bindings (imported and local, including the
    // single-letter shape) must either be capitalized alias-preservingly or
    // not JSX-ified at all — a lowercase tag is an intrinsic-element string
    // reference, not the binding.
    let input = r#"
import { jsx } from "react/jsx-runtime";
import { k } from "./dep.js";
import { widget } from "./widgets.js";
const local = ()=>null;
export const view = jsx("div", {
    children: [
        jsx(k, {}),
        jsx(widget, {}),
        jsx(local, {})
    ]
});
"#;
    let output = render(input);
    let mut offenders = Vec::new();
    for (index, _) in output.match_indices('<') {
        let tag: String = output[index + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !tag.is_empty()
            && tag.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && tag != "div"
        {
            offenders.push(tag);
        }
    }
    assert!(
        offenders.is_empty(),
        "lowercase JSX tags bind component values: {offenders:?}\n--- output ---\n{output}"
    );
}

#[test]
fn aliased_component_import_pair_stays_linked() {
    // The exact regression pair from the fixture suite: a provider exporting
    // a lowercase component under its original name, a consumer that JSX-ifies
    // it. The consumer may capitalize its local, but the edge must hold.
    let provider = r#"
const k = ({ label })=>label;
export { k };
"#;
    let consumer = r#"
import { jsx } from "react/jsx-runtime";
import { k } from "./provider.js";
export const view = jsx(k, { label: "x" });
"#;
    assert_pipeline_pair_valid(&[("provider.js", provider), ("consumer.js", consumer)]);
}

#[test]
fn mutable_export_alias_remains_distinct_and_assignable() {
    let source = r#"
var initial = new Set();
export var active = initial;
export function replace(next) {
    active = next;
}
export function readInitial() {
    return initial;
}
"#;
    let output = render(source);
    let findings = validate_output_modules(&[("provider.js".to_string(), output.clone())]);
    assert!(
        findings.is_empty(),
        "pipeline made a mutable export alias invalid: {findings:#?}\n--- output ---\n{output}"
    );
    assert!(
        output.contains("return initial;"),
        "pipeline merged an independently mutable alias with its source binding\n--- output ---\n{output}"
    );
}

#[test]
fn parameter_recovery_does_not_introduce_tdz() {
    let source = r#"
function select(source, key) {
    const { [key]: picked, ...rest } = source;
    return [picked, rest];
}
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "pipeline introduced TDZ warnings: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn object_property_parameter_recovery_does_not_introduce_tdz() {
    let source = r#"
function read(options, state) {
    var current = options.value;
    var result = current === undefined ? state.fallback : current;
    return result;
}
function choose(options) {
    var temporary = options.value;
    var value = temporary === undefined ? value ?? null : temporary;
    return value;
}
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "object-property parameter recovery introduced TDZ warnings: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn prototype_recovery_does_not_move_computed_keys_before_initialization() {
    let source = r#"
var Cursor = function(next) {
    this.next = next;
};
Cursor.prototype.toString = function() {
    return "[Cursor]";
};
var iteratorKey = getIteratorKey();
Cursor.prototype[iteratorKey] = function() {
    return this;
};
consume(iteratorKey);
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "pipeline moved a computed method key before initialization: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn value_position_rename_does_not_introduce_tdz() {
    let source = r#"
function initialize(source) {
    Object.defineProperty(source, "ready", { value: true });
    Object.keys(source);
    const v = makeSchema();
    return { Object: v };
}
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "value-position rename introduced TDZ warnings: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn parameter_rename_beside_loop_body_declaration_does_not_introduce_tdz() {
    // The loop body's `var entry` becomes a block-scoped declaration while the
    // `for` init reads the parameter. Renaming the parameter to `entry` must
    // not let the init read the body's binding before its declaration.
    let source = r#"
const api = {
  visit: function (v) {
    for (
      var queue = [{ entry: v }], index = 0;
      index < queue.length;
      index++
    ) {
      var entry = queue[index].entry;
      consume(entry);
    }
  },
};
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "parameter rename beside a loop body declaration introduced TDZ warnings: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn import_and_export_alias_recovery_does_not_introduce_tdz() {
    let source = r#"
import primary from "pkg";
import alternate from "pkg";
function read() {
    use(alternate);
    const primary = makeLocal();
    return primary;
}
use(primary);
const core = makeLogger();
const relay = core;
function report() {
    relay.error("failed");
    const logger = makeLocal();
    return logger;
}
export { relay as logger };
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "import/export alias recovery introduced TDZ warnings: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn commonjs_named_export_recovery_does_not_capture_global() {
    let source = r#"
var marker = typeof runtime !== "undefined" && runtime.pid ? runtime.pid : "";
module.exports = function() {
    return marker;
};
module.exports.runtime = function() {
    return marker;
};
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    assert!(
        output.code.contains("export { _runtime as runtime }"),
        "{}",
        output.code
    );
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "named export recovery captured a global: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn parameter_alias_recovery_does_not_capture_existing_default_reference() {
    let source = r#"
const settings = { enabled: true };
const p = !!settings.enabled;
function select(value, options = { enabled: p }) {
    let { enabled: a } = options;
    return a ? value : null;
}
consume(select);
"#;
    let output = decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            diagnostics: true,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");
    let tdz_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|warning| warning.kind == UnpackWarningKind::TdzViolation)
        .collect();
    assert!(
        tdz_warnings.is_empty(),
        "parameter alias recovery captured its default reference: {tdz_warnings:#?}\n--- output ---\n{}",
        output.code
    );
}

#[test]
fn commonjs_alias_keeps_the_default_objects_named_property() {
    let source = r#"
class Engine {}
var alias;
module.exports = alias = Engine;
module.exports.Engine = Engine;
globalThis.observed = alias.Engine === Engine;
"#;
    let output = decompile(source, DecompileOptions::default()).expect("decompile should succeed");
    assert!(
        output.code.contains(".Engine ="),
        "the property write used through the alias disappeared:\n{}",
        output.code
    );
    assert!(
        output.code.contains(".Engine ==="),
        "the alias observation must remain:\n{}",
        output.code
    );
    assert!(
        !output.code.contains("module.exports"),
        "the proven CommonJS default should recover:\n{}",
        output.code
    );
}

#[test]
fn compressed_index_reads_recover_complete_destructuring() {
    // TypeScript 5.9.3 importHelpers + Terser compress: the element bindings
    // have been inlined into the return; recovery must restore the full pattern.
    let input = r#"
Object.defineProperty(exports,"__esModule",{value:!0}),exports.read=read;
var tslib_1=require("tslib");
function read(items){var _a=tslib_1.__read(items,2),a,b;return _a[0]+_a[1]}
"#;
    let output = render(input);
    assert!(!output.contains(".__read(items, 2)"), "{output}");
    assert!(
        output.contains("function read([") || output.contains("] = items;"),
        "{output}"
    );
}

#[test]
fn export_specifier_alias_remains_independently_assignable() {
    let provider = "const initial = 1; let active = initial; export { active as Current }; export function replace(next) { active = next; } export function readInitial() { return initial; }";
    let consumer = "import { Current, replace, readInitial } from './provider.js'; use(Current, replace, readInitial);";
    assert_pipeline_pair_valid(&[("provider.js", provider), ("consumer.js", consumer)]);
}

#[test]
fn rest_assignment_does_not_capture_outer_const() {
    let source = r#"
import omit from "@babel/runtime/helpers/objectWithoutProperties";
const picked = 42;
export function extract(source) {
    var picked = source.key;
    source = omit(source, ["key"]);
    return [picked, source];
}
use(picked);
"#;
    assert_pipeline_pair_valid(&[("provider.js", source)]);
}

#[test]
fn for_of_nested_write_does_not_become_const() {
    let source = r#"
export function remap(input) {
    var out = {};
    for (var i = 0, keys = Object.keys(input); i < keys.length; i++) {
        var key = keys[i];
        out[key = key.toUpperCase()] = input[key];
    }
    return out;
}
"#;
    assert_pipeline_pair_valid(&[("provider.js", source)]);
    let output = render(source);
    assert!(
        output.contains("for (let key of Object.keys(input))"),
        "{output}"
    );
}

#[test]
fn class_temporary_initialization_recovers_direct_export() {
    // The private-field pass leaves this shape after consuming the WeakMap
    // initialization in a TypeScript class-expression factory.
    let source = "let temp; temp = class { #x = 1; getX() { return this.#x; } setX(value) { this.#x = value; } }; export const Foo = temp;";
    let expected = "export const Foo = class { #x = 1; getX() { return this.#x; } setX(value) { this.#x = value; } };";
    common::assert_eq_normalized(&render(source), expected);
    assert_pipeline_pair_valid(&[("provider.js", source)]);
}

#[test]
fn object_property_functions_stay_constructible_at_every_level() {
    // Method shorthand drops [[Construct]] and `prototype`, and no analysis
    // can prove that a property value is never constructed: the construction
    // can sit behind a helper (`extend`, `_createClass`) or in another module.
    let source = r#"
var Base = { extend: function (props) { return props; } };
var Word = Base.extend({ init: function (hi) { this.hi = hi; } });
var registry = { make: function () { return 1; } };
module.exports = [new Word.init(1).hi, registry.make()];
"#;
    for level in [
        wakaru_core::RewriteLevel::Minimal,
        wakaru_core::RewriteLevel::Standard,
        wakaru_core::RewriteLevel::Aggressive,
    ] {
        let output = common::render_with_level(source, level);
        for property in ["extend: function", "init: function", "make: function"] {
            assert!(output.contains(property), "{level:?}: {property}\n{output}");
        }
    }
}
