//! Pure output-path computation for unpacker module filenames.
//!
//! These functions translate decompiled module filenames (which originate from
//! untrusted bundle contents) into safe relative paths and disambiguate case
//! collisions. They perform no filesystem I/O — the genuine fs operations
//! (directory creation, symlink-resolving canonicalization, and file writes)
//! that wrap these helpers live in the CLI, since they are an application
//! concern and the only other core consumer (wasm) runs in the browser.
//!
//! [`safe_relative_module_path`] performs a lexical safety check, rejecting any
//! `..` traversal, absolute path, or platform prefix before the path ever
//! touches the filesystem. The CLI then performs a second, filesystem-level
//! canonicalization pass to catch symlink escapes a lexical check cannot.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};

/// Lexically validate a module filename and return it as a safe relative path.
///
/// Rejects `..` traversal, absolute paths, root, and platform path prefixes
/// (e.g. Windows drive letters). `.` components are dropped. An empty result is
/// also rejected.
pub fn safe_relative_module_path(filename: &str) -> Result<PathBuf> {
    let mut relative = PathBuf::new();
    for component in Path::new(filename).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe module filename {filename:?}: path escapes output directory")
            }
        }
    }

    if relative.as_os_str().is_empty() {
        bail!("unsafe module filename {filename:?}: empty output path");
    }

    Ok(relative)
}

/// Return a path that hasn't been used yet, disambiguating case collisions.
///
/// `seen` stores the lowercased string representation of every path already
/// claimed.  When a collision is detected the stem gets a numeric suffix:
/// `foo.js` → `foo_2.js` → `foo_3.js` …
pub fn deduplicate_path(path: &Path, seen: &mut HashSet<String>) -> PathBuf {
    let key = path.to_string_lossy().to_lowercase();
    if seen.insert(key) {
        return path.to_path_buf();
    }
    // Collision — append _N before the extension.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("js");
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut n = 2u32;
    loop {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        let candidate_key = candidate.to_string_lossy().to_lowercase();
        if seen.insert(candidate_key) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_module_path_rejects_parent_components() {
        let err = safe_relative_module_path("../escape.js").expect_err("parent should reject");
        assert!(err.to_string().contains("path escapes output directory"));
    }

    #[test]
    fn safe_relative_module_path_rejects_absolute_paths() {
        let absolute = format!(
            "{}tmp{}escape.js",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );
        let err = safe_relative_module_path(&absolute).expect_err("absolute should reject");
        assert!(err.to_string().contains("path escapes output directory"));
    }

    #[test]
    fn safe_relative_module_path_strips_current_dir_components() {
        let path = safe_relative_module_path("./src/./index.js").expect("relative path is safe");
        assert_eq!(path, PathBuf::from("src").join("index.js"));
    }

    #[test]
    fn safe_relative_module_path_rejects_empty() {
        let err = safe_relative_module_path(".").expect_err("empty should reject");
        assert!(err.to_string().contains("empty output path"));
    }

    #[test]
    fn deduplicate_path_appends_numeric_suffix_on_collision() {
        let mut seen = HashSet::new();
        let first = deduplicate_path(Path::new("src/index.js"), &mut seen);
        let second = deduplicate_path(Path::new("src/index.js"), &mut seen);
        let third = deduplicate_path(Path::new("src/index.js"), &mut seen);

        assert_eq!(first, PathBuf::from("src/index.js"));
        assert_eq!(second, PathBuf::from("src/index_2.js"));
        assert_eq!(third, PathBuf::from("src/index_3.js"));
    }

    #[test]
    fn deduplicate_path_is_case_insensitive() {
        let mut seen = HashSet::new();
        let first = deduplicate_path(Path::new("src/Index.js"), &mut seen);
        let second = deduplicate_path(Path::new("src/index.js"), &mut seen);

        assert_eq!(first, PathBuf::from("src/Index.js"));
        assert_eq!(second, PathBuf::from("src/index_2.js"));
    }
}
