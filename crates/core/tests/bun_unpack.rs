mod common;

use common::render;
use wakaru_core::{unpack, unpack_raw, DecompileOptions};

fn expect_unpack_raw(source: &str) -> Vec<(String, String)> {
    let output =
        unpack_raw(source, &DecompileOptions::default()).expect("unpack_raw should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    output.modules
}

fn module<'a>(modules: &'a [(String, String)], name: &str) -> &'a str {
    modules
        .iter()
        .find(|(filename, _)| filename == name)
        .map(|(_, code)| code.as_str())
        .unwrap_or_else(|| {
            panic!(
                "missing {name}; modules: {:?}",
                modules
                    .iter()
                    .map(|(filename, _)| filename)
                    .collect::<Vec<_>>()
            )
        })
}

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
        "pure ESM bundle should produce 1 module (no structural boundary markers)"
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

#[test]
fn bun_path_comment_names_detected_commonjs_factory() {
    let input = r#"
var __commonJS = (cb, mod) => () => (mod || cb((mod = { exports: {} }).exports, mod), mod.exports);

// src/cjs.js
var require_cjs = __commonJS((exports, module) => {
  module.exports = { cjsValue: 7, cjsFn(x) {
    return x + 1;
  } };
});

// src/entry.ts
var cjs = require_cjs();
console.log(cjs.cjsFn(cjs.cjsValue));
"#;

    let modules = expect_unpack_raw(input);
    assert!(
        modules.iter().any(|(name, _)| name == "src/cjs.js"),
        "path comment should name the structurally detected factory module: {:?}",
        modules.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        modules.iter().all(|(name, _)| name != "require_cjs.js"),
        "factory variable fallback should not be used when a direct path hint exists: {:?}",
        modules.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    let cjs = module(&modules, "src/cjs.js");
    assert!(
        cjs.contains("cjsValue") && cjs.contains("cjsFn"),
        "factory body should stay with the structurally detected module:\n{cjs}"
    );
}
