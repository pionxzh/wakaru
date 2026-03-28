use std::fs;
use wakaru_rs::unpack_webpack4_raw;

/// Snapshot test: every extracted module's PRE-RULE code is pinned.
/// This captures the output after webpack normalization but before any
/// decompile rules (SimplifySequence, UnEsm, etc.) are applied.
/// Useful for debugging rule interactions.
#[test]
fn webpack4_raw_snapshots() {
    let source_path = "../testcases/webpack4/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read webpack4 testcase");

    let mut pairs = unpack_webpack4_raw(&source).expect("raw unpack should succeed");

    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (filename, code) in &pairs {
        let snap_name = format!("raw_{}", filename.trim_end_matches(".js"));
        insta::assert_snapshot!(snap_name, code);
    }
}
