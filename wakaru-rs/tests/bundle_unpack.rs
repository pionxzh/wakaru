use std::fs;

use wakaru_rs::{unpack, DecompileOptions};

#[test]
fn webpack5_unpack_extracts_multiple_modules() {
    let source_path = "../testcases/webpack5/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read webpack5 testcase");

    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
        },
    )
    .expect("webpack5 unpack should succeed");

    assert!(
        pairs.len() > 1,
        "expected webpack5 unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected webpack5 unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}

#[test]
fn browserify_unpack_extracts_multiple_modules() {
    let source_path = "../testcases/browserify/dist/index.js";
    let source = fs::read_to_string(source_path).expect("failed to read browserify testcase");

    let pairs = unpack(
        &source,
        DecompileOptions {
            filename: source_path.to_string(),
        },
    )
    .expect("browserify unpack should succeed");

    assert!(
        pairs.len() > 1,
        "expected browserify unpack to split modules, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        pairs.iter().any(|(name, _)| name == "entry.js"),
        "expected browserify unpack to include entry.js, got {:?}",
        pairs.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
}
