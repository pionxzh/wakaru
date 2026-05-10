use std::fs;

use wakaru_core::{unpack, DecompileOptions};

fn fixture(path: &str) -> String {
    let full = format!("tests/bundles/webpack-gen/dist/{path}");
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("failed to read {full}: {e}"))
}

fn unpack_fixture(path: &str) -> Vec<(String, String)> {
    let source = fixture(path);
    unpack(
        &source,
        DecompileOptions {
            filename: path.to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|_| panic!("unpack should succeed for {path}"))
}

fn filenames(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(n, _)| n.as_str()).collect()
}

fn assert_has_entry(pairs: &[(String, String)], path: &str) {
    let names = filenames(pairs);
    // Entry is "entry.js" for array-form (numeric IDs), or named from key for object-form
    assert!(
        names
            .iter()
            .any(|n| n.contains("entry") || n.contains("index")),
        "{path}: expected an entry module, got {names:?}"
    );
}

fn assert_no_traversal(pairs: &[(String, String)], path: &str) {
    for (name, _) in pairs {
        assert!(
            !name.contains(".."),
            "{path}: filename {name} contains path traversal"
        );
    }
}

fn assert_no_runtime_helpers(pairs: &[(String, String)], path: &str) {
    for (name, code) in pairs {
        assert!(
            !code.contains("require.r("),
            "{path}/{name}: still has require.r"
        );
        assert!(
            !code.contains("require.d("),
            "{path}/{name}: still has require.d"
        );
    }
}

// ========================================================================
// Webpack 4 — dev mode (object form, string keys)
// ========================================================================

#[test]
fn wp4_cjs_dev() {
    let pairs = unpack_fixture("wp4-cjs/bundle.js");
    assert_eq!(pairs.len(), 3, "wp4-cjs: {}", filenames(&pairs).join(", "));
    assert_has_entry(&pairs, "wp4-cjs");
    assert_no_traversal(&pairs, "wp4-cjs");
}

#[test]
fn wp4_esm_dev() {
    let pairs = unpack_fixture("wp4-esm/bundle.js");
    assert_eq!(pairs.len(), 3, "wp4-esm: {}", filenames(&pairs).join(", "));
    assert_has_entry(&pairs, "wp4-esm");
    assert_no_runtime_helpers(&pairs, "wp4-esm");
}

#[test]
fn wp4_mixed_dev() {
    let pairs = unpack_fixture("wp4-mixed/bundle.js");
    assert_eq!(
        pairs.len(),
        3,
        "wp4-mixed: {}",
        filenames(&pairs).join(", ")
    );
    assert_has_entry(&pairs, "wp4-mixed");
}

#[test]
fn wp4_require_n_dev() {
    let pairs = unpack_fixture("wp4-require-n/bundle.js");
    assert_eq!(
        pairs.len(),
        3,
        "wp4-require-n: {}",
        filenames(&pairs).join(", ")
    );
    assert_has_entry(&pairs, "wp4-require-n");
}

// ========================================================================
// Webpack 4 — production (array form, numeric keys)
// ========================================================================

#[test]
fn wp4_cjs_min() {
    let pairs = unpack_fixture("wp4-cjs-min/bundle.js");
    assert_eq!(
        pairs.len(),
        3,
        "wp4-cjs-min: {}",
        filenames(&pairs).join(", ")
    );
    assert_has_entry(&pairs, "wp4-cjs-min");
}

#[test]
fn wp4_cjs_min_snapshots() {
    let mut pairs = unpack_fixture("wp4-cjs-min/bundle.js");
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (filename, code) in &pairs {
        let snap_name = format!("wp4_cjs_min__{}", filename.trim_end_matches(".js"));
        insta::assert_snapshot!(snap_name, code);
    }
}

// ========================================================================
// Webpack 4 — dynamic import (JSONP chunks)
// ========================================================================

#[test]
fn wp4_dynamic_main_bundle() {
    let pairs = unpack_fixture("wp4-dynamic/bundle.js");
    assert!(
        pairs.len() >= 2,
        "wp4-dynamic main: expected >=2 modules, got {}",
        pairs.len()
    );
    assert_has_entry(&pairs, "wp4-dynamic");
}

#[test]
fn wp4_dynamic_chunk() {
    let source = fixture("wp4-dynamic/0.bundle.js");
    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: "0.bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("wp4 JSONP chunk should unpack");
    assert_eq!(
        pairs.len(),
        1,
        "wp4 chunk: {}",
        filenames(&pairs).join(", ")
    );
}

#[test]
fn wp4_dynamic_min_chunk() {
    let source = fixture("wp4-dynamic-min/1.bundle.js");
    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: "1.bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("wp4 minified JSONP chunk should unpack");
    assert_eq!(
        pairs.len(),
        1,
        "wp4 min chunk: {}",
        filenames(&pairs).join(", ")
    );
}

// ========================================================================
// Webpack 4 — var injection
// ========================================================================

#[test]
fn wp4_var_inject() {
    let pairs = unpack_fixture("wp4-var-inject/bundle.js");
    assert!(
        pairs.len() >= 2,
        "wp4-var-inject: expected >=2, got {}",
        pairs.len()
    );
    assert_has_entry(&pairs, "wp4-var-inject");
}

// ========================================================================
// Webpack 5 — dev mode (string keys)
// ========================================================================

#[test]
fn wp5_cjs_dev() {
    let pairs = unpack_fixture("wp5-cjs/bundle.js");
    assert!(
        pairs.len() >= 3,
        "wp5-cjs: expected >=3, got {} — {}",
        pairs.len(),
        filenames(&pairs).join(", ")
    );
}

#[test]
fn wp5_esm_dev() {
    let pairs = unpack_fixture("wp5-esm/bundle.js");
    assert!(
        pairs.len() >= 3,
        "wp5-esm: expected >=3, got {} — {}",
        pairs.len(),
        filenames(&pairs).join(", ")
    );
    assert_no_runtime_helpers(&pairs, "wp5-esm");
}

#[test]
fn wp5_mixed_dev() {
    let pairs = unpack_fixture("wp5-mixed/bundle.js");
    assert!(
        pairs.len() >= 3,
        "wp5-mixed: expected >=3, got {} — {}",
        pairs.len(),
        filenames(&pairs).join(", ")
    );
}

// ========================================================================
// Webpack 5 — production (numeric keys, minified)
// ========================================================================

#[test]
fn wp5_cjs_min() {
    let pairs = unpack_fixture("wp5-cjs-min/bundle.js");
    assert!(
        pairs.len() >= 2,
        "wp5-cjs-min: expected >=2, got {} — {}",
        pairs.len(),
        filenames(&pairs).join(", ")
    );
}

#[test]
fn wp5_cjs_min_snapshots() {
    let mut pairs = unpack_fixture("wp5-cjs-min/bundle.js");
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (filename, code) in &pairs {
        let snap_name = format!("wp5_cjs_min__{}", filename.trim_end_matches(".js"));
        insta::assert_snapshot!(snap_name, code);
    }
}

// ========================================================================
// Webpack 5 — dynamic import (async chunks)
// ========================================================================

#[test]
fn wp5_dynamic_main_bundle() {
    let pairs = unpack_fixture("wp5-dynamic/bundle.js");
    assert!(
        pairs.len() >= 2,
        "wp5-dynamic main: expected >=2, got {} — {}",
        pairs.len(),
        filenames(&pairs).join(", ")
    );
}

// ========================================================================
// Webpack 5 — require.s entry (hand-crafted)
// ========================================================================

#[test]
fn wp5_require_s_entry() {
    let pairs = unpack_fixture("wp5-require-s/bundle.js");
    assert_eq!(
        pairs.len(),
        2,
        "wp5-require-s: {}",
        filenames(&pairs).join(", ")
    );
    let names = filenames(&pairs);
    let has_entry = pairs
        .iter()
        .any(|(name, _)| name == "entry.js" || name.contains("entry") || name == "module-2.js");
    assert!(
        has_entry,
        "wp5-require-s: expected entry module, got {names:?}"
    );
}

#[test]
fn wp5_require_o_entry() {
    let source = fixture("wp5-require-o/bundle.js");
    assert!(
        source.contains(".O(void 0") && source.contains("=>"),
        "wp5-require-o fixture should contain webpack's require.O arrow startup"
    );

    let pairs = unpack_fixture("wp5-require-o/bundle.js");
    assert_eq!(
        pairs.len(),
        1,
        "wp5-require-o: {}",
        filenames(&pairs).join(", ")
    );
    assert!(
        pairs.iter().any(|(_, code)| code.contains("entry:")),
        "wp5-require-o: expected extracted entry module, got {:?}",
        filenames(&pairs)
    );
}

// ========================================================================
// Path traversal (hand-crafted)
// ========================================================================

#[test]
fn wp_path_traversal_sanitized() {
    let pairs = unpack_fixture("wp-path-traversal/bundle.js");
    assert!(!pairs.is_empty(), "wp-path-traversal should unpack");
    assert_no_traversal(&pairs, "wp-path-traversal");
}

// ========================================================================
// Snapshot tests for key variants
// ========================================================================

#[test]
fn wp4_cjs_dev_snapshots() {
    let mut pairs = unpack_fixture("wp4-cjs/bundle.js");
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (filename, code) in &pairs {
        let snap_name = format!(
            "wp4_cjs_dev__{}",
            filename.replace('/', "_").trim_end_matches(".js")
        );
        insta::assert_snapshot!(snap_name, code);
    }
}

#[test]
fn wp5_cjs_dev_snapshots() {
    let mut pairs = unpack_fixture("wp5-cjs/bundle.js");
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (filename, code) in &pairs {
        let snap_name = format!(
            "wp5_cjs_dev__{}",
            filename.replace('/', "_").trim_end_matches(".js")
        );
        insta::assert_snapshot!(snap_name, code);
    }
}
