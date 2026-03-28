use std::fs;
use std::path::{Path, PathBuf};

mod common;

use wakaru_rs::{decompile, DecompileOptions};

#[test]
fn decompile_handles_existing_bundled_fixtures() {
    for fixture in bundled_fixture_paths() {
        let input = fs::read_to_string(&fixture).expect("fixture should be readable");
        let filename = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index.js")
            .to_string();

        let output =
            decompile(&input, DecompileOptions { filename }).expect("decompile should succeed");
        assert!(
            !output.trim().is_empty(),
            "output should not be empty for {}",
            fixture.display()
        );
    }
}

#[test]
fn decompile_output_is_stable_for_noop_pipeline() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testcases")
        .join("browserify")
        .join("dist")
        .join("index.js");

    let input = fs::read_to_string(&fixture).expect("fixture should be readable");
    let once = decompile(
        &input,
        DecompileOptions {
            filename: "index.js".to_string(),
        },
    )
    .expect("first decompile should succeed");
    let twice = decompile(
        &once,
        DecompileOptions {
            filename: "index.js".to_string(),
        },
    )
    .expect("second decompile should succeed");

    assert_eq!(common::normalize(&once), common::normalize(&twice));
}

#[test]
fn decompile_parses_jsx_in_js_files() {
    let output = decompile(
        "const view = <div className=\"ok\" />;",
        DecompileOptions {
            filename: "view.js".to_string(),
        },
    )
    .expect("jsx in .js should parse");

    assert!(output.contains("<div"));
}

fn bundled_fixture_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("testcases");
    let mut paths = Vec::new();

    for entry in fs::read_dir(root).expect("testcases should exist") {
        let entry = entry.expect("testcase entry should be readable");
        let path = entry.path().join("dist").join("index.js");
        if path.exists() {
            paths.push(path);
        }
    }

    paths.sort();
    paths
}
