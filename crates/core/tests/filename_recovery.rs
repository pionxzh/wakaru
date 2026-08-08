//! Filename recovery from `@sentry/babel-plugin-component-annotate` provenance
//! markers. In a bundle, a Sentry-annotated component compiles to a
//! `data-sentry-source-file` string-literal property in the `jsx`/`createElement`
//! props object. In unpack mode we harvest that value (Phase 1, pre-UnJsx) and
//! use it to rename the extracted module's output file, rewriting importers'
//! import-source strings to match.

use wakaru_core::driver::test_support::{unpack_files, UnpackInput};
use wakaru_core::{validate_output_modules, DecompileOptions, RewriteLevel};

/// A Browserify bundle containing a generated module whose JSX carries the
/// Sentry source-file marker, plus an entry that requires it by its provisional
/// filename. Physical plain inputs now reserve their authored path, so positive
/// filename-recovery coverage belongs on generated bundle outputs.
fn sentry_annotated_inputs() -> Vec<UnpackInput> {
    browserify_inputs(
        r#"
exports.Comp = function Comp() {
    return _jsx("div", {
        "data-sentry-component": "MyAwesomeComponent",
        "data-sentry-source-file": "myAwesomeComponent.jsx",
        children: "hi"
    });
}
"#,
        r#"module.exports = require("./a.js");"#,
    )
}

fn browserify_inputs(module_source: &str, entry_source: &str) -> Vec<UnpackInput> {
    vec![UnpackInput {
        filename: "bundle.js".to_string(),
        source: format!(
            r#"
(function() {{ return function() {{}}; }})()({{
  1: [function(require, module, exports) {{
    {entry_source}
  }}, {{ "./a.js": 2 }}],
  2: [function(require, module, exports) {{
    {module_source}
  }}, {{}}]
}}, {{}}, [1]);
"#
        ),
    }]
}

fn plain_sentry_annotated_inputs() -> Vec<UnpackInput> {
    vec![
        UnpackInput {
            filename: "components/a.js".to_string(),
            source: r#"
export function Comp() {
    return _jsx("div", {
        "data-sentry-component": "MyAwesomeComponent",
        "data-sentry-source-file": "myAwesomeComponent.jsx",
        children: "hi"
    });
}
"#
            .to_string(),
        },
        UnpackInput {
            filename: "consumer.js".to_string(),
            source: r#"import { Comp } from "./components/a.js";
export const x = Comp;
"#
            .to_string(),
        },
    ]
}

#[test]
fn recovers_filename_from_data_sentry_source_file() {
    let output = unpack_files(sentry_annotated_inputs(), DecompileOptions::default())
        .expect("two modules should unpack");

    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();

    assert!(
        names.contains(&"myAwesomeComponent.jsx"),
        "the annotated module should be renamed to its recovered source filename, got {names:?}"
    );
    assert!(
        !names.contains(&"a.js"),
        "the provisional filename should be replaced by the recovered one, got {names:?}"
    );
}

#[test]
fn rewrites_importer_source_to_recovered_filename() {
    let output = unpack_files(sentry_annotated_inputs(), DecompileOptions::default())
        .expect("two modules should unpack");

    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");

    assert!(
        consumer.contains("myAwesomeComponent.jsx"),
        "importer should reference the recovered filename:\n{consumer}"
    );
    assert!(
        !consumer.contains("./a.js"),
        "importer should no longer reference the provisional filename:\n{consumer}"
    );
}

#[test]
fn recovered_filename_is_used_in_source_map_file_field() {
    let output = unpack_files(
        sentry_annotated_inputs(),
        DecompileOptions {
            emit_source_map: true,
            ..Default::default()
        },
    )
    .expect("two modules should unpack with source maps");

    let map_json = output
        .source_maps
        .iter()
        .find(|(filename, _)| filename == "myAwesomeComponent.jsx")
        .map(|(_, map)| map)
        .expect("renamed module should have a source map");
    let sm = sourcemap::SourceMap::from_reader(map_json.as_bytes()).expect("source map parses");

    assert_eq!(
        sm.get_file(),
        Some("myAwesomeComponent.jsx"),
        "source map file field should match the emitted module name"
    );
}

#[test]
fn rewrites_surviving_require_source_to_recovered_filename() {
    let output = unpack_files(sentry_annotated_inputs(), DecompileOptions::default())
        .expect("bundle modules should unpack");
    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "entry.js")
        .map(|(_, code)| code)
        .expect("Browserify entry should exist");

    assert!(
        consumer.contains(r#"require("./myAwesomeComponent.jsx")"#),
        "surviving require() should reference the recovered filename:\n{consumer}"
    );
    assert!(
        !consumer.contains(r#"require("./a.js")"#),
        "surviving require() should not reference the provisional filename:\n{consumer}"
    );
}

#[test]
fn source_file_without_component_marker_does_not_recover_filename() {
    let output = unpack_files(
        browserify_inputs(
            r#"
exports.marker = {
    "data-sentry-source-file": "plainObject.jsx",
    children: "hi"
};
"#,
            r#"module.exports = require("./a.js");"#,
        ),
        DecompileOptions::default(),
    )
    .expect("bundle modules should unpack");

    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"a.js") && !names.contains(&"plainObject.jsx"),
        "source-file without component marker should not rename the module, got {names:?}"
    );
}

#[test]
fn minimal_level_keeps_provisional_filename() {
    let output = unpack_files(
        sentry_annotated_inputs(),
        DecompileOptions {
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("two modules should unpack");

    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"a.js"),
        "minimal level should not rename files from provenance markers, got {names:?}"
    );
}

#[test]
fn plain_input_public_path_is_not_recovered_from_sentry_marker() {
    let output = unpack_files(plain_sentry_annotated_inputs(), DecompileOptions::default())
        .expect("plain physical inputs should keep their reserved paths");

    let names: Vec<&str> = output.modules.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"components/a.js"), "{names:?}");
    assert!(!names.contains(&"myAwesomeComponent.jsx"), "{names:?}");
    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "consumer.js")
        .map(|(_, code)| code)
        .expect("consumer.js should exist");
    assert!(consumer.contains("./components/a.js"), "{consumer}");
    let findings = validate_output_modules(&output.modules);
    assert!(
        findings.is_empty(),
        "unexpected graph findings: {findings:#?}"
    );
}
