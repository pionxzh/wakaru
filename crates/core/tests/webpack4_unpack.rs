use std::fs;

use wakaru_core::{unpack, unpack_raw, DecompileOptions};

#[test]
fn webpack4_unpack_extracts_modules() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    // Must extract at least 50 modules
    assert!(
        pairs.len() >= 50,
        "expected at least 50 modules, got {}",
        pairs.len()
    );

    // Each module must have non-empty code
    for (filename, code) in &pairs {
        assert!(!code.trim().is_empty(), "module {filename} has empty code");
    }

    // The entry module must exist
    let has_entry = pairs.iter().any(|(name, _)| name == "entry.js");
    assert!(
        has_entry,
        "no entry.js module found; filenames: {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

#[test]
fn webpack4_raw_unpack_extracts_modules_without_pipeline() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output =
        unpack_raw(&source, &DecompileOptions::default()).expect("raw unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;

    assert!(
        pairs.len() >= 50,
        "expected at least 50 modules, got {}",
        pairs.len()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "no entry.js module found; filenames: {:?}",
        pairs.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().all(|(_, code)| !code.trim().is_empty()),
        "raw unpack should not produce empty modules"
    );

    let decompiled_output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("decompiled unpack should succeed");
    assert!(
        !decompiled_output.has_errors(),
        "unexpected warnings: {:?}",
        decompiled_output.warnings
    );
    let decompiled_pairs = decompiled_output.modules;

    assert!(
        pairs
            .iter()
            .any(|(filename, raw_code)| decompiled_pairs.iter().any(
                |(decompiled_filename, decompiled_code)| filename == decompiled_filename
                    && raw_code != decompiled_code
            )),
        "raw unpack should preserve at least one pre-pipeline module difference"
    );
}

/// Snapshot test: every extracted module's decompiled output is pinned.
/// When rule changes affect the output, `cargo test` will fail and show a diff.
/// Run `cargo insta review` to accept improvements or reject regressions.
#[test]
fn webpack4_unpack_snapshots() {
    let source_path = "../../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let output = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let mut pairs = output.modules;

    // Sort for stable snapshot order
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (filename, code) in &pairs {
        // Use the filename (without extension) as the snapshot name
        let snap_name = filename.trim_end_matches(".js");
        insta::assert_snapshot!(snap_name, code);
    }
}

#[test]
fn webpack4_unpacks_array_concat_offset_modules() {
    // webpack renders `Array(minId).concat([...])` when dense numeric module
    // ids start above zero (Template.getModulesArrayBounds); element indices
    // are offset by minId.
    let source = r#"
!function(modules) {
    var installedModules = {};
    function __webpack_require__(moduleId) {
        if (installedModules[moduleId]) {
            return installedModules[moduleId].exports;
        }
        var module = installedModules[moduleId] = { i: moduleId, l: false, exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        module.l = true;
        return module.exports;
    }
    return __webpack_require__(__webpack_require__.s = 30);
}(Array(30).concat([
    function(module, exports, __webpack_require__) {
        var dep = __webpack_require__(31);
        module.exports = dep.value + 1;
    },
    function(module, exports) {
        exports.value = 41;
    }
]));
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    // Module id 30 is the entry (require(30) in the bootstrap); id 31 follows.
    assert!(
        filenames.contains(&"entry.js") && filenames.contains(&"module-31.js"),
        "concat-form ids should be offset by Array(n), got {filenames:?}"
    );

    let entry = pairs
        .iter()
        .find(|(name, _)| name == "entry.js")
        .expect("entry.js should exist");
    assert!(
        entry.1.contains("./module-31.js"),
        "entry should reference ./module-31.js, got:\n{}",
        entry.1
    );
}

#[test]
fn webpack4_unpacks_parenthesized_array_factories() {
    // Unminified webpack 4 wraps each factory in parens: `/***/ (function(...) {...})`.
    let source = r#"
(function(modules) {
    var installedModules = {};
    function __webpack_require__(moduleId) {
        if (installedModules[moduleId]) {
            return installedModules[moduleId].exports;
        }
        var module = installedModules[moduleId] = { i: moduleId, l: false, exports: {} };
        modules[moduleId].call(module.exports, module, module.exports, __webpack_require__);
        module.l = true;
        return module.exports;
    }
    return __webpack_require__(__webpack_require__.s = 0);
})([
    (function(module, exports, __webpack_require__) {
        var dep = __webpack_require__(1);
        module.exports = dep.value * 2;
    }),
    (function(module, exports) {
        exports.value = 21;
    })
]);
"#;

    let output = unpack(
        source,
        DecompileOptions {
            filename: "bundle.js".to_string(),
            ..Default::default()
        },
    )
    .expect("unpack should succeed");
    assert!(
        !output.has_errors(),
        "unexpected warnings: {:?}",
        output.warnings
    );
    let pairs = output.modules;
    let filenames: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();

    assert!(
        filenames.contains(&"entry.js") && filenames.contains(&"module-1.js"),
        "parenthesized factories should be recognized, got {filenames:?}"
    );
}
