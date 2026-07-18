use wakaru_core::unpacker::{metro, try_unpack_bundle, BundleFormat};
use wakaru_core::{unpack, unpack_raw, DecompileOptions};

// Reduced from Metro 0.87's serializer/transform-worker output and runtime
// contract (`facebook/metro` at d2730e67d). IDs are deliberately synthetic.
const NUMERIC_BUNDLE: &str = r#"
var __DEV__ = false;

__d(function(g, r, i, a, m, e, d) {
  "use strict";
  var cjs = r(d[0]);
  var defaultValue = i(d[1]);
  var namespace = a(d[2]);
  e.result = [cjs.label, defaultValue, namespace.answer];
  e.namespace = namespace;
}, 100, [200, 201, 202], "src/index.js");

__d(function(global, require, importDefault, importAll, module, exports, dependencyMap) {
  module.exports = { label: "commonjs" };
}, 200, [], "src/cjs.js");

__d(function(global, require, importDefault, importAll, module, exports, dependencyMap) {
  Object.defineProperty(exports, "__esModule", { value: true });
  exports.default = "default-value";
}, 201, [], "src/default.js");

__d(function(global, require, importDefault, importAll, module, exports, dependencyMap) {
  Object.defineProperty(exports, "__esModule", { value: true });
  exports.answer = 42;
}, 202, [], "src/namespace.js");

__r(100);
"#;

const GENERATED_METRO_BUNDLES: [(&str, &str); 2] = [
    ("dev", include_str!("bundles/metro-gen/dist/dev.bundle.js")),
    ("min", include_str!("bundles/metro-gen/dist/min.bundle.js")),
];

#[test]
fn detects_and_extracts_numeric_metro_modules() {
    let result = try_unpack_bundle(NUMERIC_BUNDLE)
        .expect("Metro detection should not error")
        .expect("Metro bundle should be detected");

    assert_eq!(result.format, BundleFormat::Metro);
    assert_eq!(result.modules.len(), 4);
    assert!(result.modules.iter().any(|module| module.is_entry));
    assert!(result
        .modules
        .iter()
        .any(|module| module.filename == "entry.js"));
}

#[test]
fn extracts_generated_dynamic_metro_bundles() {
    for (variant, source) in GENERATED_METRO_BUNDLES {
        let result = try_unpack_bundle(source)
            .unwrap_or_else(|error| panic!("{variant} Metro detection errored: {error}"))
            .unwrap_or_else(|| panic!("{variant} Metro bundle should be detected"));
        assert_eq!(result.format, BundleFormat::Metro, "variant: {variant}");
        assert!(
            result
                .modules
                .iter()
                .any(|module| module.is_entry && module.filename == "entry.js"),
            "{variant} bundle should recognize Metro's unprefixed run statement"
        );

        let output = unpack_raw(source, &DecompileOptions::default())
            .unwrap_or_else(|error| panic!("{variant} raw unpack failed: {error}"));
        let entry = output
            .modules
            .iter()
            .find(|(name, _)| name == "entry.js")
            .map(|(_, code)| code)
            .unwrap_or_else(|| panic!("{variant} entry should exist"));
        assert!(
            entry.contains("var __metroDependencyMap =")
                && entry.contains("__metroDependencyMap.paths")
                && entry.contains(".prefetch(")
                && entry.contains(".unstable_importMaybeSync("),
            "{variant} dynamic Metro forms need a bound dependency map:\n{entry}"
        );
    }
}

#[test]
fn raw_unpack_normalizes_factory_params_and_dependency_indices() {
    let output = unpack_raw(NUMERIC_BUNDLE, &DecompileOptions::default())
        .expect("raw Metro unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry module should exist");

    assert!(
        entry.contains("require(\"./module-200.js\")"),
        "Metro require should resolve through the dependency map:\n{entry}"
    );
    assert!(
        entry.contains("import defaultValue from \"./module-201.js\""),
        "Metro default loader should become a default import:\n{entry}"
    );
    assert!(
        entry.contains("import * as namespace from \"./module-202.js\""),
        "Metro namespace loader should become a namespace import:\n{entry}"
    );
    assert!(
        !entry.contains("d[") && !entry.contains("dependencyMap[") && !entry.contains("importAll"),
        "factory-only dependency helpers should not survive raw extraction:\n{entry}"
    );
}

#[test]
fn full_unpack_recovers_imports_and_exports() {
    let output = unpack(
        NUMERIC_BUNDLE,
        DecompileOptions {
            filename: "metro.bundle".to_string(),
            ..Default::default()
        },
    )
    .expect("Metro unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry module should exist");

    assert!(
        entry.contains("import ") && entry.contains("./module-200.js"),
        "CommonJS dependency should recover as an import:\n{entry}"
    );
    assert!(
        entry.contains("./module-201.js"),
        "default Metro dependency should recover as an import:\n{entry}"
    );
    assert!(
        entry.contains("import * as namespace from \"./module-202.js\"")
            && entry.contains("namespace.answer"),
        "namespace Metro dependency should recover as a namespace import:\n{entry}"
    );
    assert!(
        !entry.contains("__d") && !entry.contains("__r") && !entry.contains("dependencyMap"),
        "Metro runtime markers should not survive decompilation:\n{entry}"
    );

    let default_module = output
        .modules
        .iter()
        .find(|(name, _)| name == "module-201.js")
        .map(|(_, code)| code)
        .expect("default-export module should exist");
    assert!(
        default_module.contains("export default"),
        "Metro exports should flow through normal ESM recovery:\n{default_module}"
    );
}

#[test]
fn supports_string_module_ids_and_matching_global_prefix() {
    let source = r#"
metro$__d(function(g, r, i, a, m, e, d) {
  m.exports = r(d[0]);
}, "src/index.js", ["src/value.js"]);
metro$__d((g, r, i, a, m, e, d) => {
  m.exports = "ok";
}, "src/value.js", []);
__r("src/index.js");
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("string-id Metro unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );

    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("string-id entry should exist");
    assert!(
        entry.contains("require(\"./src/value.js\")"),
        "string dependency ids should resolve to sanitized output paths:\n{entry}"
    );
    assert!(output
        .modules
        .iter()
        .any(|(name, _)| name == "src/value.js"));
}

#[test]
fn also_accepts_a_custom_prefixed_run_statement() {
    let source = r#"
metro$__d(function(g, r, i, a, m, e, d) {
  m.exports = "ok";
}, 1, []);
metro$__r(1);
"#;

    let result = try_unpack_bundle(source)
        .expect("prefixed Metro detection should not error")
        .expect("prefixed custom run statement should be accepted");
    assert!(result.modules[0].is_entry);
    assert_eq!(result.modules[0].filename, "entry.js");
}

#[test]
fn preserves_dependency_map_for_dynamic_and_weak_uses() {
    // Metro 0.87 emits these shapes for import(), __prefetchImport,
    // require.unstable_importMaybeSync(), and require.resolveWeak().
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  var load = r(d[0])(d[1], d.paths, "lazy");
  var prefetch = r(d[0]).prefetch(d[1], d.paths, "lazy");
  var maybeSync = r(d[0]).unstable_importMaybeSync(d[1], d.paths, "lazy");
  var weak = d[1];
  m.exports = { load: load, prefetch: prefetch, maybeSync: maybeSync, weak: weak };
}, 1, { 0: 2, 1: 3, paths: { 3: "/lazy.bundle?modulesOnly=true&runModule=false" } });
__d(function(g, r, i, a, m, e, d) { m.exports = function() {}; }, 2, []);
__d(function(g, r, i, a, m, e, d) { m.exports = "lazy"; }, 3, []);
__r(1);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("dynamic Metro raw unpack should succeed");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry should exist");

    assert!(
        entry.contains("var __metroDependencyMap = {")
            && entry.contains("paths: {")
            && entry.contains("/lazy.bundle?modulesOnly=true&runModule=false"),
        "residual dependency-map uses need a local binding with paths metadata:\n{entry}"
    );
    assert!(
        entry.contains("require(\"./module-2.js\")(")
            && entry.contains("__metroDependencyMap[1]")
            && entry.contains("__metroDependencyMap.paths"),
        "dynamic import runtime calls should remain executable:\n{entry}"
    );
    assert!(
        entry.contains(".prefetch(") && entry.contains(".unstable_importMaybeSync("),
        "prefetch and maybe-sync shapes should be preserved:\n{entry}"
    );
    assert!(
        entry.contains("weak = __metroDependencyMap[1]"),
        "resolveWeak's standalone module-id lookup should stay bound:\n{entry}"
    );
}

#[test]
fn rejects_a_factory_with_unbound_dependency_map_uses() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  m.exports = d[0];
}, 1);
__r(1);
"#;

    assert!(
        metro::detect_and_extract(source).is_none(),
        "a missing dependency map must not produce an unbound factory parameter"
    );
}

#[test]
fn supports_indexable_dependency_maps_with_async_paths_metadata() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  m.exports = r(d[0]);
}, 1, { 0: 2, paths: { 2: "/value.bundle?modulesOnly=true" } });
__d(function(g, r, i, a, m, e, d) {
  m.exports = "ok";
}, 2, []);
__r(1);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("object dependency-map Metro unpack should succeed");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry should exist");
    assert!(
        entry.contains("require(\"./module-2.js\")"),
        "numeric properties should be read from an indexable dependency map:\n{entry}"
    );
}

#[test]
fn dependency_rewrite_is_binding_aware() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  function readLocal(__metroDependencyMap) {
    return r(__metroDependencyMap[0]);
  }
  m.exports = [r(d[0]), readLocal];
}, 1, [2]);
__d(function(g, r, i, a, m, e, d) {
  m.exports = "ok";
}, 2, []);
__r(1);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("binding-aware Metro unpack should succeed");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry should exist");
    assert!(entry.contains("require(\"./module-2.js\")"));
    assert!(
        entry.contains("require(__metroDependencyMap[0])"),
        "a shadowing local dependency map must not be rewritten:\n{entry}"
    );
}

#[test]
fn preserves_writable_metro_import_helper_temporary() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  var value = i(d[0]);
  value = "replacement";
  m.exports = value;
}, 1, [2]);
__d(function(g, r, i, a, m, e, d) {
  m.exports = { default: "original" };
}, 2, []);
__r(1);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("writable Metro helper temporary should unpack");
    let entry = output
        .modules
        .iter()
        .find(|(name, _)| name == "entry.js")
        .map(|(_, code)| code)
        .expect("entry should exist");
    assert!(
        !entry.contains("import ")
            && entry.contains(r#"require("./module-2.js").default"#)
            && entry.contains("value = \"replacement\""),
        "a written helper temporary must remain a mutable local:\n{entry}"
    );
}

#[test]
fn names_multiple_entry_modules_stably() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) { m.exports = 1; }, 10, []);
__d(function(g, r, i, a, m, e, d) { m.exports = 2; }, 20, []);
__r(10);
__r(20);
"#;

    let result = try_unpack_bundle(source)
        .expect("Metro detection should not error")
        .expect("Metro bundle should be detected");
    let names = result
        .modules
        .iter()
        .map(|module| module.filename.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["entry-10.js", "entry-20.js"]);
    assert!(result.modules.iter().all(|module| module.is_entry));
}

#[test]
fn deduplicates_colliding_metro_output_filenames() {
    let source = r#"
__d(function(g, r, i, a, m, e, d) {
  m.exports = [r(d[0]), r(d[1]), r(d[2])];
}, 1, ["entry.js", 2, "module-2.js"]);
__d(function(g, r, i, a, m, e, d) { m.exports = "named entry collision"; }, "entry.js", []);
__d(function(g, r, i, a, m, e, d) { m.exports = "numeric module"; }, 2, []);
__d(function(g, r, i, a, m, e, d) { m.exports = "named module collision"; }, "module-2.js", []);
__r(1);
"#;

    let output = unpack_raw(source, &DecompileOptions::default())
        .expect("colliding Metro filenames should unpack");
    let names = output
        .modules
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["entry.js", "entry-2.js", "module-2.js", "module-2-2.js"]
    );
    let entry = &output.modules[0].1;
    assert!(
        entry.contains(r#"require("./entry-2.js")"#)
            && entry.contains(r#"require("./module-2.js")"#)
            && entry.contains(r#"require("./module-2-2.js")"#),
        "dependency rewrites must use deduplicated filenames:\n{entry}"
    );
}

#[test]
fn rejects_unrelated_d_calls() {
    let source = r#"
function __d(factory, id, values) {
  return factory(id, values);
}
__d(function(value) { return value; }, 1, []);
__r(1);
"#;

    assert!(
        try_unpack_bundle(source)
            .expect("detection should not error")
            .is_none(),
        "an unrelated __d call without Metro's factory signature must not match"
    );
}

#[test]
fn rejects_partial_selected_prefix_with_malformed_definition() {
    let source = r#"
const sharedDependencyMap = [];
__d(function(g, r, i, a, m, e, d) {
  m.exports = r(d[0]);
}, 1, [2]);
__d(function(g, r, i, a, m, e, d) {
  m.exports = "value";
}, 2, sharedDependencyMap);
__r(1);
"#;

    assert!(
        metro::detect_and_extract(source).is_none(),
        "a malformed definition must reject the whole Metro candidate instead of emitting a dangling import"
    );
}

#[test]
fn reports_factory_body_provenance() {
    let output = unpack_raw(
        NUMERIC_BUNDLE,
        &DecompileOptions {
            filename: "metro.bundle".to_string(),
            ..Default::default()
        },
    )
    .expect("raw Metro unpack should succeed");

    let entry = output
        .provenance
        .iter()
        .find(|entry| entry.filename == "entry.js")
        .expect("entry provenance should exist");
    let text = entry
        .ranges
        .iter()
        .map(|&(start, end)| &NUMERIC_BUNDLE[start as usize..end as usize])
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains("var cjs = r(d[0])"),
        "range should cover the factory body"
    );
    assert!(
        !text.contains("__r(100)"),
        "range must not include the bundle postlude"
    );
}

#[test]
fn metro_raw_snapshots() {
    let output = unpack_raw(NUMERIC_BUNDLE, &DecompileOptions::default())
        .expect("raw Metro unpack should succeed");
    let mut modules = output.modules;
    modules.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (filename, code) in modules {
        let name = format!("raw_{}", filename.trim_end_matches(".js"));
        insta::assert_snapshot!(name, code);
    }
}

#[test]
fn metro_decompiled_snapshots() {
    let output = unpack(
        NUMERIC_BUNDLE,
        DecompileOptions {
            filename: "metro.bundle".to_string(),
            ..Default::default()
        },
    )
    .expect("Metro unpack should succeed");
    let mut modules = output.modules;
    modules.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (filename, code) in modules {
        let name = format!("decompiled_{}", filename.trim_end_matches(".js"));
        insta::assert_snapshot!(name, code);
    }
}
