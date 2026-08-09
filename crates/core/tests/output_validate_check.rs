//! Tests for the emitted-module graph validator.
//!
//! The validator is a development/benchmark tool that checks normal unpack
//! output for structural defects that would make it fail to load as ESM.
//! Raw output is deliberately out of scope: raw carries no quality contract.

use wakaru_core::{validate_output_modules, OutputFindingKind};

fn modules(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, code)| (name.to_string(), code.to_string()))
        .collect()
}

fn kinds(pairs: &[(&str, &str)]) -> Vec<(OutputFindingKind, String)> {
    validate_output_modules(&modules(pairs))
        .into_iter()
        .map(|finding| (finding.kind, finding.filename))
        .collect()
}

fn locations(pairs: &[(&str, &str)]) -> Vec<(OutputFindingKind, String, usize, usize)> {
    validate_output_modules(&modules(pairs))
        .into_iter()
        .map(|finding| (finding.kind, finding.filename, finding.line, finding.column))
        .collect()
}

#[test]
fn clean_module_graph_reports_nothing() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"
import { helper } from "./util.js";
import fallback from "./default.js";
console.log(helper(), fallback);
"#,
        ),
        ("util.js", "export function helper() { return 1; }\n"),
        ("default.js", "export default 42;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn dangling_static_import_is_reported() {
    let findings = kinds(&[(
        "entry.js",
        r#"import { gone } from "./missing.js"; console.log(gone);"#,
    )]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::DanglingRelativeRef, "entry.js".into())]
    );
}

#[test]
fn findings_include_one_based_source_locations() {
    let findings = locations(&[
        (
            "entry.js",
            r#"
import { missing } from "./util.js";
missing = 1;
export const duplicate = 1;
export { missing as duplicate };
"#,
        ),
        ("util.js", "export const other = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![
            (OutputFindingKind::DuplicateExport, "entry.js".into(), 5, 10,),
            (OutputFindingKind::AssignToImport, "entry.js".into(), 3, 1,),
            (
                OutputFindingKind::MissingImportedName,
                "entry.js".into(),
                2,
                10,
            ),
        ]
    );
}

#[test]
fn dangling_reference_location_points_to_the_specifier() {
    let findings = locations(&[("entry.js", "\nimport \"./missing.js\";\n")]);
    assert_eq!(
        findings,
        vec![(
            OutputFindingKind::DanglingRelativeRef,
            "entry.js".into(),
            2,
            8,
        )]
    );
}

#[test]
fn dangling_dynamic_import_and_require_are_reported() {
    let findings = kinds(&[(
        "entry.js",
        r#"
import("./gone.js");
const lib = require("./also-gone.js");
console.log(lib);
"#,
    )]);
    assert_eq!(
        findings,
        vec![
            (OutputFindingKind::DanglingRelativeRef, "entry.js".into()),
            (OutputFindingKind::DanglingRelativeRef, "entry.js".into()),
        ]
    );
}

#[test]
fn bare_and_external_specifiers_are_ignored() {
    let findings = kinds(&[(
        "entry.js",
        r#"
import react from "react";
import("some-pkg/lazy");
const fs = require("fs");
console.log(react, fs);
"#,
    )]);
    assert_eq!(findings, vec![]);
}

#[test]
fn locally_bound_require_is_not_treated_as_module_ref() {
    let findings = kinds(&[(
        "entry.js",
        r#"
function require(spec) { return spec; }
require("./not-a-module.js");
"#,
    )]);
    assert_eq!(findings, vec![]);
}

#[test]
fn missing_named_import_is_reported() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { missing } from "./util.js"; console.log(missing);"#,
        ),
        ("util.js", "export const other = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::MissingImportedName, "entry.js".into())]
    );
}

#[test]
fn missing_default_import_is_reported() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"import util from "./util.js"; console.log(util);"#,
        ),
        ("util.js", "export const named = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::MissingImportedName, "entry.js".into())]
    );
}

#[test]
fn import_alias_checks_the_external_name() {
    // `import { une as Une }` must check `une` against the provider, not `Une`.
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { une as Une } from "./util.js"; console.log(Une);"#,
        ),
        ("util.js", "export const une = 1;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn export_star_provides_names_transitively() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { deep } from "./a.js"; console.log(deep);"#,
        ),
        ("a.js", "export * from \"./b.js\";\n"),
        ("b.js", "export const deep = 1;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn export_star_does_not_forward_default() {
    let findings = kinds(&[
        ("entry.js", r#"import a from "./a.js"; console.log(a);"#),
        ("a.js", "export * from \"./b.js\";\n"),
        ("b.js", "export default 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::MissingImportedName, "entry.js".into())]
    );
}

#[test]
fn external_star_export_suppresses_name_checks() {
    // A provider re-exporting an external package has an unknowable export
    // set; imports from it must not be flagged.
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { anything } from "./a.js"; console.log(anything);"#,
        ),
        ("a.js", "export * from \"some-pkg\";\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn reexport_of_missing_name_is_reported() {
    let findings = kinds(&[
        ("a.js", "export { nope } from \"./b.js\";\n"),
        ("b.js", "export const yep = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::MissingImportedName, "a.js".into())]
    );
}

#[test]
fn namespace_reexport_provides_its_alias() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { ns } from "./a.js"; console.log(ns);"#,
        ),
        ("a.js", "export * as ns from \"./b.js\";\n"),
        ("b.js", "export const inner = 1;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn duplicate_export_is_reported() {
    let findings = kinds(&[(
        "entry.js",
        r#"
export const twice = 1;
const other = 2;
export { other as twice };
"#,
    )]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::DuplicateExport, "entry.js".into())]
    );
}

#[test]
fn assign_to_import_is_reported() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"
import { helper } from "./util.js";
helper = 1;
helper++;
({ helper } = {});
"#,
        ),
        ("util.js", "export let helper = 0;\n"),
    ]);
    assert_eq!(
        findings,
        vec![
            (OutputFindingKind::AssignToImport, "entry.js".into()),
            (OutputFindingKind::AssignToImport, "entry.js".into()),
            (OutputFindingKind::AssignToImport, "entry.js".into()),
        ]
    );
}

#[test]
fn namespace_import_write_is_reported() {
    let findings = kinds(&[
        ("entry.js", r#"import * as ns from "./util.js"; ns = 1;"#),
        ("util.js", "export const x = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::AssignToImport, "entry.js".into())]
    );
}

#[test]
fn shadowed_import_write_is_not_reported() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"
import { helper } from "./util.js";
function local(helper) {
    helper = 1;
    return helper;
}
console.log(helper, local(2));
"#,
        ),
        ("util.js", "export const helper = 0;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn assign_to_const_is_reported() {
    let findings = kinds(&[(
        "entry.js",
        r#"
const top = 1;
top = 2;
function nested() {
    const inner = 1;
    inner += 1;
}
nested();
"#,
    )]);
    assert_eq!(
        findings,
        vec![
            (OutputFindingKind::AssignToConst, "entry.js".into()),
            (OutputFindingKind::AssignToConst, "entry.js".into()),
        ]
    );
}

#[test]
fn let_reassignment_is_not_reported() {
    let findings = kinds(&[(
        "entry.js",
        r#"
let counter = 0;
counter = 1;
counter++;
for (const item of [1, 2]) {
    console.log(item, counter);
}
"#,
    )]);
    assert_eq!(findings, vec![]);
}

#[test]
fn for_of_write_to_const_binding_is_scoped_correctly() {
    // `for (const item of ...)` re-binds per iteration; only a write inside
    // the body to that same binding is invalid.
    let findings = kinds(&[(
        "entry.js",
        r#"
for (const item of [1, 2]) {
    item = 3;
}
"#,
    )]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::AssignToConst, "entry.js".into())]
    );
}

#[test]
fn parse_error_is_reported() {
    let findings = locations(&[("entry.js", "const ok = 1;\nfunction {\n")]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::ParseError, "entry.js".into(), 2, 10,)]
    );
}

#[test]
fn sloppy_script_output_falls_back_to_script_goal() {
    // Single-file decompile output can legitimately be a classic script;
    // module-goal-only constructs like `with` must not be parse findings.
    let findings = kinds(&[(
        "entry.js",
        r#"
var scope = { x: 1 };
with (scope) {
    console.log(x);
}
"#,
    )]);
    assert_eq!(findings, vec![]);
}

#[test]
fn jsx_in_js_output_parses_and_participates_in_graph() {
    // Standard-level UnJsx emits JSX syntax in .js files; the validator must
    // parse it and still catch graph defects in the same file.
    let findings = kinds(&[
        (
            "app.js",
            r#"
import { widget } from "./missing.js";
export function App() {
    return <div className="x">{widget}</div>;
}
"#,
        ),
        ("util.js", "export const helper = 1;\n"),
    ]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::DanglingRelativeRef, "app.js".into())]
    );
}

#[test]
fn extensionless_module_files_resolve() {
    // Some emitted trees carry extensionless module filenames.
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { helper } from "./util"; console.log(helper);"#,
        ),
        ("util", "export const helper = 1;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn extensionless_relative_ref_resolves_to_js_sibling() {
    let findings = kinds(&[
        (
            "entry.js",
            r#"import { helper } from "./util"; console.log(helper);"#,
        ),
        ("util.js", "export const helper = 1;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn nested_directory_references_resolve() {
    let findings = kinds(&[
        (
            "src/entry.js",
            r#"import { a } from "./lib/a.js"; import { b } from "../b.js"; console.log(a, b);"#,
        ),
        ("src/lib/a.js", "export const a = 1;\n"),
        ("b.js", "export const b = 2;\n"),
    ]);
    assert_eq!(findings, vec![]);
}

#[test]
fn relative_ref_escaping_the_root_is_dangling() {
    let findings = kinds(&[(
        "entry.js",
        r#"import { x } from "../outside.js"; console.log(x);"#,
    )]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::DanglingRelativeRef, "entry.js".into())]
    );
}

#[test]
fn name_checks_are_skipped_for_dangling_targets() {
    // A missing module already produces a dangling finding; don't stack a
    // missing-name finding on top of it.
    let findings = kinds(&[(
        "entry.js",
        r#"import { x } from "./missing.js"; console.log(x);"#,
    )]);
    assert_eq!(
        findings,
        vec![(OutputFindingKind::DanglingRelativeRef, "entry.js".into())]
    );
}
