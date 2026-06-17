//! Filename recovery from `@sentry/babel-plugin-component-annotate` provenance
//! markers. In a bundle, a Sentry-annotated component compiles to a
//! `data-sentry-source-file` string-literal property in the `jsx`/`createElement`
//! props object. In unpack mode we harvest that value (Phase 1, pre-UnJsx) and
//! use it to rename the extracted module's output file, rewriting importers'
//! import-source strings to match.

use wakaru_core::{unpack_files, DecompileOptions, RewriteLevel, UnpackInput};

/// A component module whose JSX carries the Sentry source-file marker in its
/// pre-JSX (props-object) form, plus a consumer module that imports it by its
/// provisional filename.
fn sentry_annotated_inputs() -> Vec<UnpackInput> {
    vec![
        UnpackInput {
            filename: "a.js".to_string(),
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
            filename: "b.js".to_string(),
            source: r#"import { Comp } from "./a.js";
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
        .find(|(n, _)| n == "b.js")
        .map(|(_, code)| code)
        .expect("consumer module b.js should exist");

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
fn rewrites_surviving_require_source_to_recovered_filename() {
    let mut inputs = sentry_annotated_inputs();
    inputs[1].source = r#"export default require("./a.js");"#.to_string();

    let output =
        unpack_files(inputs, DecompileOptions::default()).expect("two modules should unpack");
    let consumer = output
        .modules
        .iter()
        .find(|(n, _)| n == "b.js")
        .map(|(_, code)| code)
        .expect("consumer module b.js should exist");

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
        vec![
            UnpackInput {
                filename: "a.js".to_string(),
                source: r#"
export const marker = {
    "data-sentry-source-file": "plainObject.jsx",
    children: "hi"
};
"#
                .to_string(),
            },
            UnpackInput {
                filename: "b.js".to_string(),
                source: r#"import { marker } from "./a.js";
export const x = marker;
"#
                .to_string(),
            },
        ],
        DecompileOptions::default(),
    )
    .expect("two modules should unpack");

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
