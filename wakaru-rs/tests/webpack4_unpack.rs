use std::fs;

use wakaru_rs::{unpack, DecompileOptions};

#[test]
fn webpack4_unpack_extracts_modules() {
    let source_path = "../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
        },
    )
    .expect("unpack should succeed");

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

/// Snapshot test: every extracted module's decompiled output is pinned.
/// When rule changes affect the output, `cargo test` will fail and show a diff.
/// Run `cargo insta review` to accept improvements or reject regressions.
#[test]
fn webpack4_unpack_snapshots() {
    let source_path = "../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path)
        .expect("failed to read webpack4 testcase — make sure the testcases are present");

    let mut pairs = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
        },
    )
    .expect("unpack should succeed");

    // Sort for stable snapshot order
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (filename, code) in &pairs {
        // Use the filename (without extension) as the snapshot name
        let snap_name = filename.trim_end_matches(".js");
        insta::assert_snapshot!(snap_name, code);
    }
}
