//! Recursive input-directory discovery for `--unpack`.
//!
//! This module only performs filesystem traversal. Candidates are pushed into
//! `wakaru::UnpackJob` by the caller so detection is not repeated and skipped
//! source text can be released immediately.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Counts produced by a directory scan: how many candidate files were read,
/// how many were detected as bundle/chunk inputs, and how many were skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryScanStats {
    pub scanned: usize,
    pub detected: usize,
    pub skipped: usize,
}

/// Recursively collect `.js`/`.mjs`/`.cjs` files under `root`, skipping hidden
/// entries and `node_modules`. Results are sorted by their string path.
pub fn collect_directory_js_inputs(root: &Path) -> Result<Vec<PathBuf>> {
    collect_sorted(root, is_js_like_input)
}

/// Like [`collect_directory_js_inputs`] but for `debug validate`: emitted
/// module trees can carry `.jsx` and extensionless filenames, so those are
/// accepted too. Source maps and other extensions stay excluded.
pub fn collect_validate_inputs(root: &Path) -> Result<Vec<PathBuf>> {
    collect_sorted(root, is_emitted_module_file)
}

fn collect_sorted(root: &Path, accepts: fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_directory_js_inputs_inner(root, &mut paths, accepts)?;
    paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(paths)
}

fn collect_directory_js_inputs_inner(
    dir: &Path,
    paths: &mut Vec<PathBuf>,
    accepts: fn(&Path) -> bool,
) -> Result<()> {
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
            collect_directory_js_inputs_inner(&path, paths, accepts)?;
        } else if file_type.is_file() && !is_hidden_name(&file_name) && accepts(&path) {
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

fn is_emitted_module_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "js" | "mjs" | "cjs" | "jsx"
        ),
        None => true,
    }
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
    fn collect_validate_inputs_accepts_jsx_and_extensionless() {
        let dir = temp_test_dir("validate");
        fs::create_dir_all(&dir).expect("create dir");

        fs::write(dir.join("a.js"), "1").expect("write js");
        fs::write(dir.join("b.jsx"), "1").expect("write jsx");
        fs::write(dir.join("module"), "1").expect("write extensionless");
        fs::write(dir.join("map.js.map"), "1").expect("write source map (ignored)");
        fs::write(dir.join(".hidden"), "1").expect("write hidden (ignored)");

        let collected = collect_validate_inputs(&dir).expect("collect");
        let names: Vec<String> = collected
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.js", "b.jsx", "module"]);

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }
}
