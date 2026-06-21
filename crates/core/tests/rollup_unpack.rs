mod common;
use common::render;
use wakaru_core::{unpack, DecompileOptions};

// --- Decompile (single-file) tests ---

#[test]
fn rollup_es_decompiles() {
    let input = include_str!("bundles/rollup-gen/dist/es/bundle.mjs");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("main"), "exported function should survive");
    insta::assert_snapshot!("rollup_es_decompile", output);
}

#[test]
fn rollup_es_min_decompiles() {
    let input = include_str!("bundles/rollup-gen/dist/es-min/bundle.mjs");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("export"), "exports should be present");
    insta::assert_snapshot!("rollup_es_min_decompile", output);
}

// --- Unpack tests ---

#[test]
fn rollup_es_unpack_behavior() {
    let input = include_str!("bundles/rollup-gen/dist/es/bundle.mjs");
    let opts = DecompileOptions {
        filename: "bundle.mjs".into(),
        ..Default::default()
    };
    let result = unpack(input, opts);
    match result {
        Ok(output) => {
            eprintln!(
                "Rollup ES bundle: detected, {} modules extracted.",
                output.modules.len()
            );
            for (filename, code) in &output.modules {
                eprintln!("  module: {} ({} bytes)", filename, code.len());
                assert!(!code.trim().is_empty(), "module code should not be empty");
            }
        }
        Err(e) => {
            eprintln!("Rollup ES bundle: NOT detected as bundle format: {e}");
        }
    }
}

#[test]
fn rollup_es_min_unpack_behavior() {
    let input = include_str!("bundles/rollup-gen/dist/es-min/bundle.mjs");
    let opts = DecompileOptions {
        filename: "bundle.mjs".into(),
        ..Default::default()
    };
    let result = unpack(input, opts);
    match result {
        Ok(output) => {
            eprintln!(
                "Rollup ES-min bundle: detected, {} modules.",
                output.modules.len()
            );
            for (filename, code) in &output.modules {
                eprintln!("  module: {} ({} bytes)", filename, code.len());
                assert!(!code.trim().is_empty());
            }
        }
        Err(e) => {
            eprintln!("Rollup ES-min bundle: NOT detected: {e}");
        }
    }
}
