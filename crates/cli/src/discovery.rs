//! Recursive input-directory scanning for `--unpack`.
//!
//! When the unpacker is pointed at a directory, it is expanded recursively to
//! `.js`/`.mjs`/`.cjs` candidates while skipping hidden files/directories and
//! `node_modules`. Each candidate is then *detected* via core's
//! `is_detected_unpack_input`: only files that match a bundle/chunk shape are
//! kept. Non-matching files are skipped rather than copied or decompiled.
//!
//! The directory walking lives in the CLI because it is genuine filesystem I/O
//! (an application concern); the bundle-shape detection it relies on stays in
//! `wakaru-core` (the decompiler's domain) and is reused here.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wakaru_core::{is_detected_unpack_input, UnpackInput};

/// Counts produced by a directory scan: how many candidate files were read,
/// how many were detected as bundle/chunk inputs, and how many were skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryScanStats {
    pub scanned: usize,
    pub detected: usize,
    pub skipped: usize,
}

/// Recursively scan `root` for detected bundle/chunk inputs.
///
/// Returns the detected inputs (sorted by discovered path) alongside scan
/// statistics. Hidden files/directories and `node_modules` are skipped, and
/// only `.js`/`.mjs`/`.cjs` files are considered.
pub fn scan_directory_for_unpack_inputs(
    root: &Path,
    heuristic_split: bool,
) -> Result<(Vec<UnpackInput>, DirectoryScanStats)> {
    let mut inputs = Vec::new();
    let mut stats = DirectoryScanStats::default();

    for path in collect_directory_js_inputs(root)? {
        stats.scanned += 1;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if is_detected_unpack_input(&source, heuristic_split) {
            stats.detected += 1;
            inputs.push(UnpackInput {
                filename: path.to_string_lossy().to_string(),
                source,
            });
        } else {
            stats.skipped += 1;
        }
    }

    Ok((inputs, stats))
}

/// Recursively collect `.js`/`.mjs`/`.cjs` files under `root`, skipping hidden
/// entries and `node_modules`. Results are sorted by their string path.
pub fn collect_directory_js_inputs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_directory_js_inputs_inner(root, &mut paths)?;
    paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(paths)
}

fn collect_directory_js_inputs_inner(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read input directory {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read input directory {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_dir() {
            if is_hidden_name(&file_name) || file_name == "node_modules" {
                continue;
            }
            collect_directory_js_inputs_inner(&path, paths)?;
        } else if file_type.is_file() && !is_hidden_name(&file_name) && is_js_like_input(&path) {
            paths.push(path);
        }
    }

    Ok(())
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

fn is_js_like_input(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wakaru-cli-discovery-test-{name}-{nanos}"))
    }

    fn webpack5_chunk_source() -> &'static str {
        r#"
(self.webpackChunk = self.webpackChunk || []).push([
  [1],
  {
    100: function(module, exports, require) {
      "use strict";
      require.r(exports);
      exports.default = 1;
    }
  }
]);
"#
    }

    fn webpack5_runtime_entry_source() -> &'static str {
        r#"
(() => {
  var modules = {};
  function require(id) { return {}; }
  require.m = modules;
  require.f = {};
  require.e = function(id) { return Promise.resolve(id); };
  require.u = function(id) { return id + ".bundle.js"; };
  require.t = function(value) { return value; };
  require.e(529).then(require.t.bind(require, 529, 19));
})();
"#
    }

    fn runtime_like_plain_source() -> &'static str {
        r#"
(() => {
  const api = {};
  api.e = 1;
  api.u = 2;
  api.t = 3;
  api.m = 4;
})();
"#
    }

    #[test]
    fn detects_webpack_chunk_source() {
        assert!(is_detected_unpack_input(webpack5_chunk_source(), false));
    }

    #[test]
    fn rejects_plain_source() {
        assert!(!is_detected_unpack_input("const value = 1;", true));
    }

    #[test]
    fn scan_keeps_only_detected_js_and_skips_hidden_and_node_modules() {
        let dir = temp_test_dir("scan");
        let nested = dir.join("nested");
        let hidden = dir.join(".hidden");
        let node_modules = dir.join("node_modules");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::create_dir_all(&hidden).expect("create hidden dir");
        fs::create_dir_all(&node_modules).expect("create node_modules dir");

        fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");
        fs::write(dir.join("runtime-like.js"), runtime_like_plain_source())
            .expect("write runtime-like plain file");
        fs::write(nested.join("chunk.js"), webpack5_chunk_source()).expect("write chunk");
        fs::write(dir.join("runtime.js"), webpack5_runtime_entry_source())
            .expect("write runtime entry");
        fs::write(hidden.join("hidden.js"), webpack5_chunk_source()).expect("write hidden chunk");
        fs::write(node_modules.join("vendor.js"), webpack5_chunk_source())
            .expect("write node_modules chunk");
        // Non-js-like extension must be ignored entirely (not even scanned).
        fs::write(dir.join("chunk.js.map"), webpack5_chunk_source()).expect("write sourcemap");

        let (inputs, stats) =
            scan_directory_for_unpack_inputs(&dir, false).expect("scan directory");
        assert_eq!(
            stats,
            DirectoryScanStats {
                scanned: 4,
                detected: 2,
                skipped: 2,
            }
        );
        assert_eq!(inputs.len(), 2, "expected visible chunk and runtime entry");
        assert!(
            inputs
                .iter()
                .any(|input| input.filename.ends_with("nested\\chunk.js")
                    || input.filename.ends_with("nested/chunk.js")),
            "missing detected chunk input: {inputs:?}"
        );
        assert!(
            inputs
                .iter()
                .any(|input| input.filename.ends_with("runtime.js")),
            "missing detected runtime input: {inputs:?}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn collect_honors_extensions_and_filters() {
        let dir = temp_test_dir("collect");
        let hidden = dir.join(".git");
        let node_modules = dir.join("node_modules");
        fs::create_dir_all(&hidden).expect("create hidden dir");
        fs::create_dir_all(&node_modules).expect("create node_modules dir");

        fs::write(dir.join("a.js"), "1").expect("write js");
        fs::write(dir.join("b.mjs"), "1").expect("write mjs");
        fs::write(dir.join("c.cjs"), "1").expect("write cjs");
        fs::write(dir.join("d.ts"), "1").expect("write ts (ignored)");
        fs::write(dir.join("e.txt"), "1").expect("write txt (ignored)");
        fs::write(dir.join(".hidden.js"), "1").expect("write hidden file");
        fs::write(hidden.join("inner.js"), "1").expect("write hidden dir file");
        fs::write(node_modules.join("pkg.js"), "1").expect("write node_modules file");

        let collected = collect_directory_js_inputs(&dir).expect("collect");
        let names: Vec<String> = collected
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.js", "b.mjs", "c.cjs"]);

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn scan_empty_directory_reports_zero_stats() {
        let dir = temp_test_dir("empty");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("plain.js"), "const value = 1;").expect("write plain file");

        let (inputs, stats) =
            scan_directory_for_unpack_inputs(&dir, false).expect("scan directory");
        assert!(inputs.is_empty());
        assert_eq!(
            stats,
            DirectoryScanStats {
                scanned: 1,
                detected: 0,
                skipped: 1,
            }
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }
}
