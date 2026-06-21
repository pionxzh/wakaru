mod common;
use common::render;
use wakaru_core::{unpack, DecompileOptions};

// --- Decompile (single-file) tests ---

#[test]
fn bun_es_decompiles() {
    let input = include_str!("bundles/bun-gen/dist/es/entry.js");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("main"), "exported function should survive");
    insta::assert_snapshot!("bun_es_decompile", output);
}

#[test]
fn bun_es_min_decompiles() {
    let input = include_str!("bundles/bun-gen/dist/es-min/entry.js");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("export"), "exports should be present");
    insta::assert_snapshot!("bun_es_min_decompile", output);
}

#[test]
fn bun_cjs_interop_decompiles() {
    let input = include_str!("bundles/bun-gen/dist/cjs-interop/entry-cjs.js");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("main"), "exported function should survive");
    insta::assert_snapshot!("bun_cjs_interop_decompile", output);
}

#[test]
fn bun_cjs_interop_min_decompiles() {
    let input = include_str!("bundles/bun-gen/dist/cjs-interop-min/entry-cjs.js");
    let output = render(input);
    assert!(!output.trim().is_empty());
    assert!(output.contains("export"), "exports should be present");
    insta::assert_snapshot!("bun_cjs_interop_min_decompile", output);
}

// --- Unpack tests ---

#[test]
fn bun_es_unpack_produces_single_module() {
    let input = include_str!("bundles/bun-gen/dist/es/entry.js");
    let opts = DecompileOptions {
        filename: "entry.js".into(),
        ..Default::default()
    };
    let result = unpack(input, opts).expect("unpack should succeed");
    assert_eq!(
        result.modules.len(),
        1,
        "pure ESM bundle should produce 1 module (no boundary markers)"
    );
}

#[test]
fn bun_cjs_interop_unpack_splits_modules() {
    let input = include_str!("bundles/bun-gen/dist/cjs-interop/entry-cjs.js");
    let opts = DecompileOptions {
        filename: "entry-cjs.js".into(),
        ..Default::default()
    };
    let result = unpack(input, opts).expect("unpack should succeed");
    assert!(
        result.modules.len() >= 3,
        "CJS-interop bundle should split into at least 3 modules, got {}",
        result.modules.len()
    );
    for (filename, code) in &result.modules {
        assert!(!code.trim().is_empty(), "{filename} should not be empty");
    }
    let combined: String = result
        .modules
        .iter()
        .map(|(name, code)| format!("// === {name} ===\n{code}"))
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!("bun_cjs_interop_unpack", combined);
}

#[test]
fn bun_cjs_interop_min_unpack_splits_modules() {
    let input = include_str!("bundles/bun-gen/dist/cjs-interop-min/entry-cjs.js");
    let opts = DecompileOptions {
        filename: "entry-cjs.js".into(),
        ..Default::default()
    };
    let result = unpack(input, opts).expect("unpack should succeed");
    assert!(
        result.modules.len() >= 3,
        "minified CJS-interop bundle should split into at least 3 modules, got {}",
        result.modules.len()
    );
    for (filename, code) in &result.modules {
        assert!(!code.trim().is_empty(), "{filename} should not be empty");
    }
    let combined: String = result
        .modules
        .iter()
        .map(|(name, code)| format!("// === {name} ===\n{code}"))
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!("bun_cjs_interop_min_unpack", combined);
}
