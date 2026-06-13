//! Filesystem output helpers for the CLI's `--unpack` writer.
//!
//! These functions translate decompiled module filenames (which originate from
//! untrusted bundle contents) into concrete on-disk paths that are guaranteed
//! to stay inside the chosen output directory, plus the write-if-changed logic
//! the writer wires around them.
//!
//! Pure path computation ([`safe_relative_module_path`], [`deduplicate_path`])
//! lives in `wakaru-core` and is reused here; this module owns the genuine
//! filesystem I/O (directory creation, symlink-resolving canonicalization, and
//! the file writes) that is an application concern rather than a decompiler
//! concern.
//!
//! Safety model: a module filename may contain `../` traversal, an absolute
//! path, a Windows drive prefix, or a path component that resolves through a
//! symlink pointing outside the output directory. Each of those must be
//! rejected. The two-stage approach is:
//!
//! 1. core's `safe_relative_module_path` performs a lexical check, rejecting any
//!    non-`Normal`/non-`CurDir` component before the path ever touches the
//!    filesystem.
//! 2. [`canonicalize_unpack_output_path`] walks the parent directories,
//!    creating or canonicalizing each one and confirming the canonical result
//!    still starts with the output directory. This catches symlink escapes that
//!    a purely lexical check cannot.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use wakaru_core::{deduplicate_path, safe_relative_module_path};

/// Canonicalize the output directory so later path checks compare against a
/// fully-resolved root (symlinks resolved, `.`/`..` removed).
pub fn canonicalize_output_dir(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize output directory {}", path.display()))
}

/// Resolve a module `filename` to a concrete output path under `out_dir`.
///
/// Performs the lexical safety check, deduplicates against previously claimed
/// paths in `seen`, then canonicalizes parent directories (creating them as
/// needed) and confirms the result stays inside `out_dir`.
pub fn resolve_unpack_output_path(
    out_dir: &Path,
    filename: &str,
    seen: &mut HashSet<String>,
) -> Result<PathBuf> {
    let relative = safe_relative_module_path(filename)?;
    let lexical_path = deduplicate_path(&out_dir.join(relative), seen);
    canonicalize_unpack_output_path(out_dir, &lexical_path, filename)
}

/// Canonicalize `lexical_path` (already known to be under `out_dir`
/// lexically) against the real filesystem, creating missing parent
/// directories and confirming every resolved component stays inside `out_dir`.
///
/// This is what catches symlink escapes: a directory component that is a
/// symlink pointing outside `out_dir` resolves to a path that no longer starts
/// with `out_dir`, and is rejected.
pub fn canonicalize_unpack_output_path(
    out_dir: &Path,
    lexical_path: &Path,
    filename: &str,
) -> Result<PathBuf> {
    let relative = lexical_path.strip_prefix(out_dir).with_context(|| {
        format!("unsafe module filename {filename:?}: path escapes output directory")
    })?;
    let Some(file_name) = relative.file_name() else {
        bail!("unsafe module filename {filename:?}: empty output path");
    };

    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = out_dir.to_path_buf();
    for component in parent_relative.components() {
        let Component::Normal(part) = component else {
            bail!("unsafe module filename {filename:?}: path escapes output directory");
        };
        current.push(part);
        if current.exists() {
            let canonical = current.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize output directory {}",
                    current.display()
                )
            })?;
            ensure_path_inside_output_dir(out_dir, &canonical, filename)?;
            if !canonical.is_dir() {
                bail!(
                    "output path {} exists and is not a directory",
                    current.display()
                );
            }
            current = canonical;
        } else {
            fs::create_dir(&current).with_context(|| {
                format!("failed to create output directory {}", current.display())
            })?;
            let canonical = current.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize output directory {}",
                    current.display()
                )
            })?;
            ensure_path_inside_output_dir(out_dir, &canonical, filename)?;
            current = canonical;
        }
    }

    let candidate = current.join(file_name);
    let target = if candidate.exists() {
        candidate.canonicalize().with_context(|| {
            format!("failed to canonicalize output file {}", candidate.display())
        })?
    } else {
        candidate
    };
    ensure_path_inside_output_dir(out_dir, &target, filename)?;
    Ok(target)
}

fn ensure_path_inside_output_dir(out_dir: &Path, path: &Path, filename: &str) -> Result<()> {
    if path.starts_with(out_dir) {
        Ok(())
    } else {
        bail!("unsafe module filename {filename:?}: path escapes output directory");
    }
}

/// Write `content` to `path`, replacing any existing file.
pub fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

/// Write `content` to `path` only when it differs from the existing file.
///
/// Skips the write when the on-disk file already has identical length and
/// bytes, avoiding redundant writes (and touch timestamps) when re-running
/// against an existing output directory.
pub fn write_if_changed(path: &Path, content: &str) -> Result<()> {
    if let Ok(metadata) = fs::metadata(path) {
        if metadata.len() == content.len() as u64
            && fs::read(path).is_ok_and(|existing| existing == content.as_bytes())
        {
            return Ok(());
        }
    }

    write_file(path, content)
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
        std::env::temp_dir().join(format!("wakaru-cli-output-test-{name}-{nanos}"))
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[test]
    fn unpack_output_path_rejects_parent_dir_components() {
        let dir = temp_test_dir("escape");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let err = resolve_unpack_output_path(
            &out_dir,
            "../node_modules/@wakaru/cli/bin/wakaru",
            &mut seen,
        )
        .expect_err("parent path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_keeps_overlap_payload_inside_output_dir() {
        let dir = temp_test_dir("overlap");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let path = resolve_unpack_output_path(
            &out_dir,
            "....//node_modules/@wakaru/cli/bin/wakaru",
            &mut seen,
        )
        .expect("overlapping dots are an ordinary relative directory");
        assert!(
            path.starts_with(&out_dir),
            "resolved path should stay in output dir: {}",
            path.display()
        );
        assert!(path.ends_with("node_modules/@wakaru/cli/bin/wakaru"));

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_absolute_paths() {
        let dir = temp_test_dir("absolute");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();
        let absolute = format!(
            "{}tmp{}escape.js",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        );

        let err = resolve_unpack_output_path(&out_dir, &absolute, &mut seen)
            .expect_err("absolute module path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[cfg(windows)]
    #[test]
    fn unpack_output_path_rejects_windows_drive_prefixes() {
        let dir = temp_test_dir("drive-prefix");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, r"C:\tmp\escape.js", &mut seen)
            .expect_err("drive-prefixed module path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_parent_directory_that_is_file() {
        let dir = temp_test_dir("file-parent");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("src"), "not a directory").expect("write file parent");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect_err("file parent should be rejected");
        assert!(
            err.to_string().contains("exists and is not a directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_deduplicates_after_safety_checks() {
        let dir = temp_test_dir("dedup");
        fs::create_dir_all(&dir).expect("create temp dir");
        let out_dir = canonicalize_output_dir(&dir).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let first = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect("first path should resolve");
        let second = resolve_unpack_output_path(&out_dir, "src/index.js", &mut seen)
            .expect("second path should resolve with suffix");

        assert!(first.starts_with(&out_dir), "{}", first.display());
        assert!(second.starts_with(&out_dir), "{}", second.display());
        assert_ne!(first, second);
        assert_eq!(first.file_name().and_then(|s| s.to_str()), Some("index.js"));
        assert_eq!(
            second.file_name().and_then(|s| s.to_str()),
            Some("index_2.js")
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_symlink_parent_that_points_outside() {
        let dir = temp_test_dir("symlink-parent");
        let out_dir_raw = dir.join("out");
        let external_dir = dir.join("external");
        fs::create_dir_all(&out_dir_raw).expect("create output dir");
        fs::create_dir_all(&external_dir).expect("create external dir");
        let link_path = out_dir_raw.join("link");
        if create_dir_symlink(&external_dir, &link_path).is_err() {
            fs::remove_dir_all(&dir).expect("remove temp dir");
            return;
        }
        let out_dir = canonicalize_output_dir(&out_dir_raw).expect("canonicalize output dir");
        let mut seen = HashSet::new();

        let err = resolve_unpack_output_path(&out_dir, "link/pwn.js", &mut seen)
            .expect_err("symlink parent escaping output dir should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_if_changed_skips_identical_readonly_file() {
        let dir = temp_test_dir("write-if-changed");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");
        fs::write(&path, "same").expect("write temp file");

        let original_permissions = fs::metadata(&path).expect("metadata").permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("set readonly");

        assert!(write_if_changed(&path, "same").is_ok());

        fs::set_permissions(&path, original_permissions).expect("restore permissions");
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_if_changed_overwrites_different_length_file() {
        let dir = temp_test_dir("write-if-changed-length");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");
        fs::write(&path, "short").expect("write temp file");

        write_if_changed(&path, "longer content").expect("write changed file");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated file"),
            "longer content"
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_file_creates_and_overwrites() {
        let dir = temp_test_dir("write-file");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");

        write_file(&path, "first").expect("write new file");
        assert_eq!(fs::read_to_string(&path).expect("read"), "first");

        write_file(&path, "second").expect("overwrite file");
        assert_eq!(fs::read_to_string(&path).expect("read"), "second");

        fs::remove_dir_all(&dir).expect("remove temp dir");
    }
}
