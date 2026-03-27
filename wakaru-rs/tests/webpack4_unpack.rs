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
        assert!(
            !code.trim().is_empty(),
            "module {filename} has empty code"
        );
    }

    // The entry module must exist
    let has_entry = pairs.iter().any(|(name, _)| name == "entry.js" || name.starts_with("entry-"));
    assert!(has_entry, "no entry module found; filenames: {:?}", pairs.iter().map(|(n, _)| n).collect::<Vec<_>>());
}
