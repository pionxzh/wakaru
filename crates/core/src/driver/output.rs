//! Pure output-path computation for unpacker module filenames.
//!
//! These functions translate decompiled module filenames (which originate from
//! untrusted bundle contents) into safe relative paths and disambiguate case
//! collisions. They perform no filesystem I/O — the genuine fs operations
//! (directory creation, symlink-resolving canonicalization, and file writes)
//! that wrap these helpers live in the CLI, since they are an application
//! concern and the only other core consumer (wasm) runs in the browser.
//!
//! Every bundle- or input-derived path is split by [`split_logical_path`], which
//! reads `/` and `\` as separators on every platform. Splitting with
//! `std::path` instead differs by platform: on Unix `..\..\x` is one ordinary
//! component, so a check that passes there is undone by a later rewrite of `\`
//! to `/`. Two policies sit on the one splitter:
//!
//! - [`sanitize_relative_path`] drops what cannot stay (`..`, a root, a drive)
//!   and keeps the rest, for names a producer derives from bundle text;
//! - [`safe_relative_module_name`] rejects any of them, for the final check
//!   before a name is returned or written.
//!
//! The CLI then performs a second, filesystem-level canonicalization pass to
//! catch symlink escapes a lexical check cannot.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// One segment of a path split by [`split_logical_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathSegment<'a> {
    Normal(&'a str),
    Parent,
}

/// A path split the same way on every platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LogicalPath<'a> {
    /// The path started at a root, a drive (`C:`), or a UNC or verbatim
    /// prefix (`\\server\share`, `\\?\C:`), so it names no relative location.
    pub anchored: bool,
    /// Segments in order; empty and `.` segments are already gone.
    pub segments: Vec<PathSegment<'a>>,
}

/// Split `raw` with `/` and `\` as separators, independent of the host.
///
/// A leading drive (`C:`, `C:\`) or separator marks the path anchored. After a
/// leading separator, a verbatim `?` segment and the drive after it are part
/// of the prefix (`\\?\C:\x` keeps only `x`).
pub(crate) fn split_logical_path(raw: &str) -> LogicalPath<'_> {
    let (mut anchored, rest) = match raw.as_bytes() {
        [drive, b':', ..] if drive.is_ascii_alphabetic() => (true, &raw[2..]),
        _ => (false, raw),
    };
    anchored |= rest.starts_with(['/', '\\']);
    let mut parts = rest
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .peekable();
    if anchored && parts.next_if_eq(&"?").is_some() {
        parts.next_if(|part| is_drive(part));
    }
    let segments = parts
        .map(|part| {
            if part == ".." {
                PathSegment::Parent
            } else {
                PathSegment::Normal(part)
            }
        })
        .collect();
    LogicalPath { anchored, segments }
}

fn is_drive(part: &str) -> bool {
    matches!(part.as_bytes(), [drive, b':'] if drive.is_ascii_alphabetic())
}

/// Convert a bundler-provided path into a relative output path, dropping what
/// cannot stay: `..` segments, a root, and a leading drive or UNC prefix.
///
/// This is component-based instead of replacement-based so overlapping strings
/// like `....//foo` cannot turn into `../foo` after a single sanitation pass.
/// Each kept component is also made writable on Windows (see
/// [`windows_safe_path_component`]). Returns `fallback` when nothing is left.
pub fn sanitize_relative_path(raw: &str, fallback: &str) -> String {
    let parts: Vec<String> = split_logical_path(raw)
        .segments
        .into_iter()
        .filter_map(|segment| match segment {
            PathSegment::Normal(part) => Some(windows_safe_path_component(part)),
            PathSegment::Parent => None,
        })
        .collect();
    if parts.is_empty() {
        fallback.to_string()
    } else {
        parts.join("/")
    }
}

/// Replace what Win32 rejects or silently rewrites in a path component: the
/// characters `<>:"|?*` and ASCII controls, a trailing dot or space (Win32
/// strips it, so the file on disk would not match the import specifier), and
/// a reserved device stem (`con.js`, `NUL`, `com1.txt`).
///
/// The replacement is `_`, not a `%XX` escape: emitted filenames are also
/// relative import specifiers, and ESM resolution percent-decodes a
/// specifier, so `./a%22b.js` would load `a"b.js` instead of the file
/// written. Distinct names that collapse to one spelling are separated by
/// the callers' filename deduplication.
fn windows_safe_path_component(component: &str) -> String {
    let mut safe: String = component
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0'..='\x1f' => '_',
            _ => ch,
        })
        .collect();
    if safe.ends_with(['.', ' ']) {
        safe.pop();
        safe.push('_');
    }
    if is_windows_device_stem(safe.split('.').next().unwrap_or_default()) {
        safe.insert(0, '_');
    }
    safe
}

fn is_windows_device_stem(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            upper.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && matches!(upper.as_bytes()[3], b'1'..=b'9')
        }
    }
}

/// Validate a module filename and return it as a normalized, slash-separated
/// relative name.
///
/// Rejects `..` segments, a root, and a drive or UNC prefix, written with
/// either separator on any platform. `.` and empty segments are dropped. An
/// empty result is also rejected.
pub fn safe_relative_module_name(filename: &str) -> Result<String> {
    let path = split_logical_path(filename);
    if path.anchored || path.segments.contains(&PathSegment::Parent) {
        bail!("unsafe module filename {filename:?}: path escapes output directory");
    }
    let parts: Vec<&str> = path
        .segments
        .iter()
        .filter_map(|segment| match segment {
            PathSegment::Normal(part) => Some(*part),
            PathSegment::Parent => None,
        })
        .collect();
    if parts.is_empty() {
        bail!("unsafe module filename {filename:?}: empty output path");
    }
    Ok(parts.join("/"))
}

/// [`safe_relative_module_name`] as a path, for joining below an output root.
pub fn safe_relative_module_path(filename: &str) -> Result<PathBuf> {
    safe_relative_module_name(filename).map(PathBuf::from)
}

/// Return a path that hasn't been used yet, disambiguating case collisions.
///
/// `seen` stores the lowercased string representation of every path already
/// claimed.  When a collision is detected the stem gets a numeric suffix:
/// `foo.js` → `foo_2.js` → `foo_3.js` …
pub fn deduplicate_path(path: &Path, seen: &mut std::collections::HashSet<String>) -> PathBuf {
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
    use std::collections::HashSet;

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
    fn logical_paths_split_on_both_separators_on_every_platform() {
        use PathSegment::{Normal, Parent};
        for (raw, anchored, segments) in [
            (r"..\..\x.js", false, vec![Parent, Parent, Normal("x.js")]),
            (
                "src/./a//b.js",
                false,
                vec![Normal("src"), Normal("a"), Normal("b.js")],
            ),
            (r"C:\src\a.js", true, vec![Normal("src"), Normal("a.js")]),
            ("C:a.js", true, vec![Normal("a.js")]),
            ("/tmp/x", true, vec![Normal("tmp"), Normal("x")]),
            (
                r"\\server\share\x",
                true,
                vec![Normal("server"), Normal("share"), Normal("x")],
            ),
            (r"\\?\C:\x", true, vec![Normal("x")]),
            ("src/C:", false, vec![Normal("src"), Normal("C:")]),
            ("ab:c.js", false, vec![Normal("ab:c.js")]),
            ("....//x", false, vec![Normal("...."), Normal("x")]),
        ] {
            assert_eq!(
                split_logical_path(raw),
                LogicalPath { anchored, segments },
                "{raw:?}"
            );
        }
    }

    #[test]
    fn sanitize_relative_path_drops_what_cannot_stay() {
        assert_eq!(
            sanitize_relative_path(r"C:\src\..\a.js", "m.js"),
            "src/a.js"
        );
        assert_eq!(sanitize_relative_path(r"\\?\C:\x.js", "m.js"), "x.js");
        assert_eq!(sanitize_relative_path("/../..", "m.js"), "m.js");
    }

    #[test]
    fn safe_relative_module_name_rejects_traversal_with_either_separator() {
        for unsafe_name in [
            r"..\escape.js",
            r"src\..\..\escape.js",
            r"\escape.js",
            "C:/escape.js",
            "C:escape.js",
            r"\\server\share\escape.js",
        ] {
            let err = safe_relative_module_name(unsafe_name).expect_err(unsafe_name);
            assert!(
                err.to_string().contains("path escapes output directory"),
                "{unsafe_name:?}: {err}"
            );
        }
        assert_eq!(
            safe_relative_module_name(r".\src\index.js").expect("relative name is safe"),
            "src/index.js"
        );
    }

    #[test]
    fn safe_relative_module_path_rejects_empty() {
        let err = safe_relative_module_path(".").expect_err("empty should reject");
        assert!(err.to_string().contains("empty output path"));
    }

    #[test]
    fn deduplicate_path_appends_numeric_suffix_on_collision() {
        let mut seen = HashSet::default();
        let first = deduplicate_path(Path::new("src/index.js"), &mut seen);
        let second = deduplicate_path(Path::new("src/index.js"), &mut seen);
        let third = deduplicate_path(Path::new("src/index.js"), &mut seen);

        assert_eq!(first, PathBuf::from("src/index.js"));
        assert_eq!(second, PathBuf::from("src/index_2.js"));
        assert_eq!(third, PathBuf::from("src/index_3.js"));
    }

    #[test]
    fn deduplicate_path_is_case_insensitive() {
        let mut seen = HashSet::default();
        let first = deduplicate_path(Path::new("src/Index.js"), &mut seen);
        let second = deduplicate_path(Path::new("src/index.js"), &mut seen);

        assert_eq!(first, PathBuf::from("src/Index.js"));
        assert_eq!(second, PathBuf::from("src/index_2.js"));
    }
}
