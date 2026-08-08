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
use wakaru_core::validate_output_modules;

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
