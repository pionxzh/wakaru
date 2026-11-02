//! Filesystem output for the CLI.
//!
//! Every file the CLI writes below an output directory goes through
//! [`OutputDir`]: names that come from input content (module filenames, source
//! map `sources`, Bun embedded paths) and the CLI's own files
//! (`provenance.json`, a GTM `index.md`, `.map` sidecars) alike. A path the
//! user typed on the command line goes through [`write_user_named_file`] or
//! [`create_user_named_file`] instead. Nothing else in the crate writes files:
//! `crates/cli/clippy.toml` disallows the `std::fs` writers, and only this
//! module allows them.
//!
//! Safety model: a name may contain `..`, a root, a drive or UNC prefix, or a
//! directory component that resolves through a symlink pointing outside the
//! output directory. Each of those must be rejected. Planning and writing are
//! separate steps:
//!
//! 1. [`OutputDir::plan`], [`OutputDir::plan_sidecar`] and
//!    [`OutputDir::reserve`] check a name lexically with core's
//!    `safe_relative_module_name` (which reads `/` and `\` as separators on
//!    every platform), give it a unique case-insensitive name, and refuse a
//!    name that two writers would share. They touch no filesystem, so a refusal
//!    leaves the directory as it was.
//! 2. [`OutputDir::materialize`] walks the parent directories, creating or
//!    canonicalizing each one and confirming the canonical result still starts
//!    with the output directory. This catches symlink escapes that a lexical
//!    check cannot.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use wakaru_core::safe_relative_module_name;

/// An output directory and every name planned below it.
#[derive(Debug)]
pub struct OutputDir {
    /// Fully-resolved root (symlinks resolved, `.`/`..` removed).
    root: PathBuf,
    /// Lowercased names already claimed, for case-insensitive deduplication.
    claimed: HashSet<String>,
    /// Lowercased names a later writer owns, with what owns them.
    reserved: HashMap<String, String>,
    /// Lowercased directories that claimed names sit below.
    directories: HashSet<String>,
    /// The directory already had entries, so an unchanged file is not rewritten.
    check_existing: bool,
}

/// A name planned below an [`OutputDir`], not yet on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedPath {
    /// Normalized, slash-separated relative name, after deduplication.
    name: String,
    /// The name as the caller asked for it, for error messages.
    requested: String,
}

impl PlannedPath {
    /// The relative name the file is written under.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A [`PlannedPath`] whose parent directories exist inside the output directory.
#[derive(Debug, Clone)]
pub struct OutputFile {
    path: PathBuf,
}

impl OutputFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl OutputDir {
    /// Make `path` usable as an output directory, creating it when missing.
    ///
    /// A directory that already has entries needs `force`; writes into it skip
    /// files whose bytes are unchanged.
    pub fn open(path: &Path, force: bool) -> Result<Self> {
        let check_existing = ensure_output_dir(path, force)?;
        let root = path.canonicalize().with_context(|| {
            format!("failed to canonicalize output directory {}", path.display())
        })?;
        Ok(Self {
            root,
            claimed: HashSet::new(),
            reserved: HashMap::new(),
            directories: HashSet::new(),
            check_existing,
        })
    }

    /// The canonical output root.
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Plan a name that comes from input content. A colliding name gets a
    /// `_2`, `_3`, … suffix; a name the CLI reserved for its own file is refused,
    /// and so is a name that another planned name needs as a directory.
    pub fn plan(&mut self, filename: &str) -> Result<PlannedPath> {
        let name = output_name(filename)?;
        self.refuse_reserved(&name, filename)?;
        let name = deduplicate_name(&name, &mut self.claimed);
        self.refuse_reserved(&name, filename)?;
        self.record_directories(&name, filename, None)?;
        Ok(PlannedPath {
            name,
            requested: filename.to_string(),
        })
    }

    /// Reserve `name` for a file the CLI writes itself. `owner` says what
    /// writes it, for the error when input content asks for the same name.
    pub fn reserve(&mut self, name: &str, owner: &str) -> Result<PlannedPath> {
        let name = output_name(name)?;
        let key = name.to_lowercase();
        if self.claimed.contains(&key) {
            bail!("a recovered module is named {name:?}, which {owner} also writes");
        }
        if let Some(existing) = self.reserved.get(&key) {
            bail!("{owner} and {existing} would both write {name:?}");
        }
        self.record_directories(&name, &name, Some(owner))?;
        self.claimed.insert(key.clone());
        self.reserved.insert(key, owner.to_string());
        Ok(PlannedPath {
            requested: name.clone(),
            name,
        })
    }

    /// One name cannot be both a file and a directory: refuse `name` when an
    /// earlier name sits below it, or when one of its own directories is an
    /// earlier file. Either way the second write would fail, after other
    /// files were already written. Then record `name`'s directories.
    fn record_directories(
        &mut self,
        name: &str,
        requested: &str,
        owner: Option<&str>,
    ) -> Result<()> {
        let key = name.to_lowercase();
        if self.directories.contains(&key) {
            let by = owner.map_or_else(String::new, |owner| format!(" (by {owner})"));
            bail!(
                "output name {requested:?}{by} is also the directory other output is written below"
            );
        }
        let ancestors: Vec<&str> = key
            .match_indices('/')
            .map(|(index, _)| &key[..index])
            .collect();
        if let Some(file) = ancestors
            .iter()
            .find(|ancestor| self.claimed.contains(**ancestor))
        {
            bail!(
                "output name {requested:?} is below {file:?}, which is also written as a file{}",
                self.owned_by(file)
            );
        }
        self.directories
            .extend(ancestors.into_iter().map(str::to_string));
        Ok(())
    }

    fn owned_by(&self, key: &str) -> String {
        self.reserved
            .get(key)
            .map_or_else(String::new, |owner| format!(" (by {owner})"))
    }

    /// Reserve the sidecar of `planned` named by appending `suffix`
    /// (`a.js` → `a.js.map`).
    pub fn plan_sidecar(&mut self, planned: &PlannedPath, suffix: &str) -> Result<PlannedPath> {
        let owner = format!("the {suffix} file of {:?}", planned.name);
        self.reserve(&format!("{}{suffix}", planned.name), &owner)
    }

    fn refuse_reserved(&self, name: &str, requested: &str) -> Result<()> {
        if let Some(owner) = self.reserved.get(&name.to_lowercase()) {
            bail!("a recovered module is named {requested:?}, which {owner} also writes");
        }
        Ok(())
    }

    /// Create the parent directories of `planned` and confirm that every
    /// resolved component stays inside the output directory.
    ///
    /// This is what catches symlink escapes: a directory component that is a
    /// symlink pointing outside the root resolves to a path that no longer
    /// starts with it, and is rejected. So is an existing file at the target
    /// that is a symlink to somewhere outside.
    pub fn materialize(&self, planned: &PlannedPath) -> Result<OutputFile> {
        let requested = planned.requested.as_str();
        let relative = Path::new(&planned.name);
        let Some(file_name) = relative.file_name() else {
            bail!("unsafe module filename {requested:?}: empty output path");
        };

        let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
        let mut current = self.root.clone();
        for component in parent_relative.components() {
            let Component::Normal(part) = component else {
                bail!("unsafe module filename {requested:?}: path escapes output directory");
            };
            current.push(part);
            if current.exists() {
                let canonical = canonicalize_dir(&current)?;
                self.ensure_inside(&canonical, requested)?;
                if !canonical.is_dir() {
                    bail!(
                        "output path {} exists and is not a directory",
                        current.display()
                    );
                }
                current = canonical;
            } else {
                #[allow(clippy::disallowed_methods, reason = "the one directory creator")]
                fs::create_dir(&current).with_context(|| {
                    format!("failed to create output directory {}", current.display())
                })?;
                let canonical = canonicalize_dir(&current)?;
                self.ensure_inside(&canonical, requested)?;
                current = canonical;
            }
        }

        let candidate = current.join(file_name);
        let path = if candidate.exists() {
            candidate.canonicalize().with_context(|| {
                format!("failed to canonicalize output file {}", candidate.display())
            })?
        } else {
            candidate
        };
        self.ensure_inside(&path, requested)?;
        Ok(OutputFile { path })
    }

    /// [`plan`](Self::plan) and [`materialize`](Self::materialize) one name.
    #[cfg(test)]
    pub fn claim(&mut self, filename: &str) -> Result<OutputFile> {
        let planned = self.plan(filename)?;
        self.materialize(&planned)
    }

    fn ensure_inside(&self, path: &Path, requested: &str) -> Result<()> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            bail!("unsafe module filename {requested:?}: path escapes output directory");
        }
    }

    /// Write `content` to `file`. When the directory already had entries, a
    /// file whose bytes are unchanged is left alone (and keeps its timestamp).
    pub fn write(&self, file: &OutputFile, content: &[u8]) -> Result<()> {
        if self.check_existing && file_has_bytes(&file.path, content) {
            return Ok(());
        }
        write_bytes(&file.path, content)
    }
}

/// Check a name with core's `safe_relative_module_name`, then refuse a `:` on
/// Windows.
///
/// NTFS does not write `a:b.js` as a file with that name: it writes a hidden
/// stream `b.js` of a file `a` (an alternate data stream). The write succeeds,
/// a directory listing shows only an empty `a`, and copying or archiving the
/// output can drop the content, so the name is refused instead. Other hosts
/// write the name as it is. The check rejects a leading drive first, so a `:`
/// here is always inside a segment.
fn output_name(raw: &str) -> Result<String> {
    let name = safe_relative_module_name(raw)?;
    if cfg!(windows) && name.contains(':') {
        bail!(
            "output name {raw:?} contains `:`, which Windows would write as a hidden NTFS \
             alternate data stream instead of a file; run this on Linux or macOS"
        );
    }
    Ok(name)
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize output directory {}", path.display()))
}

/// Returns true when the directory already contained entries.
fn ensure_output_dir(path: &Path, force: bool) -> Result<bool> {
    if path.exists() {
        if !path.is_dir() {
            bail!(
                "output path {} exists and is not a directory",
                path.display()
            );
        }
        let is_empty = path
            .read_dir()
            .with_context(|| format!("failed to read output directory {}", path.display()))?
            .next()
            .is_none();
        if !is_empty && !force {
            bail!(
                "output directory {} is not empty; pass --force to write into it",
                path.display()
            );
        }
        return Ok(!is_empty);
    }
    #[allow(
        clippy::disallowed_methods,
        reason = "creates the output directory the user named"
    )]
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create output directory {}", path.display()))?;
    Ok(false)
}

/// Return `name` if unclaimed, else the first free `stem_N.ext` beside it.
/// Keys are lowercased, so names differing only in case collide. A name
/// without an extension gets `.js`, as module deduplication always has.
fn deduplicate_name(name: &str, claimed: &mut HashSet<String>) -> String {
    if claimed.insert(name.to_lowercase()) {
        return name.to_string();
    }
    let (dir, file) = match name.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, name),
    };
    let (stem, extension) = file
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .unwrap_or((file, "js"));
    for n in 2u32.. {
        let file = format!("{stem}_{n}.{extension}");
        let candidate = match dir {
            Some(dir) => format!("{dir}/{file}"),
            None => file,
        };
        if claimed.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("u32 suffixes are not exhausted")
}

fn file_has_bytes(path: &Path, content: &[u8]) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() == content.len() as u64)
        && fs::read(path).is_ok_and(|existing| existing == content)
}

fn write_bytes(path: &Path, content: &[u8]) -> Result<()> {
    #[allow(clippy::disallowed_methods, reason = "the one file writer")]
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

/// Write a file at a path the user typed on the command line (`-o out.js`, or
/// a sidecar named after it). Never pass a path built from input content: that
/// goes through [`OutputDir`].
pub fn write_user_named_file(path: &Path, content: &[u8]) -> Result<()> {
    write_bytes(path, content)
}

/// Create a file at a path the user typed on the command line, such as
/// `--profile`. Never pass a path built from input content.
pub fn create_user_named_file(path: &Path) -> Result<fs::File> {
    #[allow(clippy::disallowed_methods, reason = "creates a file the user named")]
    fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, reason = "tests build their own fixtures")]
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

    fn open_empty(name: &str) -> (PathBuf, OutputDir) {
        let dir = temp_test_dir(name);
        let out = OutputDir::open(&dir, false).expect("open output dir");
        (dir, out)
    }

    fn assert_escapes(result: Result<impl std::fmt::Debug>) {
        let err = result.expect_err("path should be rejected");
        assert!(
            err.to_string().contains("path escapes output directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unpack_output_path_rejects_parent_dir_components() {
        let (dir, mut out) = open_empty("escape");
        assert_escapes(out.claim("../node_modules/@wakaru/cli/bin/wakaru"));
        assert_escapes(out.claim(r"src\..\..\escape.js"));
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_keeps_overlap_payload_inside_output_dir() {
        let (dir, mut out) = open_empty("overlap");
        let file = out
            .claim("....//node_modules/@wakaru/cli/bin/wakaru")
            .expect("overlapping dots are an ordinary relative directory");
        assert!(
            file.path().starts_with(out.root()),
            "resolved path should stay in output dir: {}",
            file.path().display()
        );
        assert!(file.path().ends_with("node_modules/@wakaru/cli/bin/wakaru"));
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_absolute_paths_and_drives() {
        let (dir, mut out) = open_empty("absolute");
        assert_escapes(out.claim("/tmp/escape.js"));
        assert_escapes(out.claim(r"\tmp\escape.js"));
        assert_escapes(out.claim(r"C:\tmp\escape.js"));
        assert_escapes(out.claim("C:escape.js"));
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_parent_directory_that_is_file() {
        let dir = temp_test_dir("file-parent");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("src"), "not a directory").expect("write file parent");
        let mut out = OutputDir::open(&dir, true).expect("open output dir");

        let err = out
            .claim("src/index.js")
            .expect_err("file parent should be rejected");
        assert!(
            err.to_string().contains("exists and is not a directory"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_deduplicates_after_safety_checks() {
        let (dir, mut out) = open_empty("dedup");
        let names: Vec<String> = [
            "src/index.js",
            "src/Index.js",
            r"src\index.js",
            "src/index_2.js",
        ]
        .into_iter()
        .map(|name| out.plan(name).expect("plan").name().to_string())
        .collect();
        assert_eq!(
            names,
            [
                "src/index.js",
                "src/Index_2.js",
                "src/index_3.js",
                "src/index_2_2.js"
            ]
        );
        let plain = out.plan("module-plain").expect("plan");
        let again = out.plan("module-plain").expect("plan");
        assert_eq!(
            (plain.name(), again.name()),
            ("module-plain", "module-plain_2.js")
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn reserved_names_refuse_modules_in_either_order() {
        let (dir, mut out) = open_empty("reserved");
        out.reserve("provenance.json", "--provenance")
            .expect("reserve provenance");
        let err = out
            .plan("Provenance.json")
            .expect_err("module may not take a reserved name");
        assert!(
            err.to_string().contains("--provenance also writes"),
            "unexpected error: {err}"
        );

        let module = out.plan("a.js").expect("plan module");
        let err = out
            .reserve("A.js", "a GTM index")
            .expect_err("reserving a claimed name fails");
        assert!(err.to_string().contains("a GTM index also writes"), "{err}");

        out.plan_sidecar(&module, ".map").expect("plan sidecar");
        let err = out
            .plan("a.js.map")
            .expect_err("module may not take its sibling's source map name");
        assert!(err.to_string().contains("the .map file of"), "{err}");
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn open_checks_existing_files_only_in_a_non_empty_dir() {
        let (empty_dir, out) = open_empty("open-new");
        assert!(!out.check_existing, "a new directory writes directly");
        fs::remove_dir_all(&empty_dir).expect("remove temp dir");

        let existing = temp_test_dir("open-empty");
        fs::create_dir_all(&existing).expect("create temp dir");
        assert!(
            !OutputDir::open(&existing, false)
                .expect("open")
                .check_existing
        );
        fs::write(existing.join("entry.js"), "old").expect("write temp file");
        assert!(
            OutputDir::open(&existing, true)
                .expect("open")
                .check_existing
        );
        fs::remove_dir_all(&existing).expect("remove temp dir");
    }

    #[test]
    fn colon_in_a_name_is_refused_only_on_windows() {
        let (dir, mut out) = open_empty("colon");
        let result = out.plan("module-node:fs.js");
        if cfg!(windows) {
            let err = result.expect_err("NTFS would hide the content in a stream");
            assert!(
                err.to_string().contains("alternate data stream"),
                "unexpected error: {err}"
            );
            assert!(out.reserve("gtm:a/index.md", "a GTM index").is_err());
        } else {
            assert_eq!(result.expect("plan").name(), "module-node:fs.js");
        }
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn a_name_cannot_be_both_a_file_and_a_directory() {
        let (dir, mut out) = open_empty("file-and-dir");
        out.reserve("provenance.json", "--provenance")
            .expect("reserve provenance");
        let err = out
            .plan("Provenance.json/x.js")
            .expect_err("a module below a reserved file");
        assert!(
            err.to_string()
                .contains("which is also written as a file (by --provenance)"),
            "{err}"
        );

        let module = out.plan("a.js").expect("plan module");
        out.plan("a.js.map/x.js")
            .expect("a directory named like a future sidecar");
        let err = out
            .plan_sidecar(&module, ".map")
            .expect_err("the sidecar name is already a directory");
        assert!(err.to_string().contains("is also the directory"), "{err}");

        out.plan("lib/util.js").expect("plan module");
        assert!(out.plan("lib").is_err(), "a module named like a directory");
        assert!(
            out.plan("lib/util.js/x.js").is_err(),
            "a module below a module"
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn planning_touches_no_filesystem() {
        let (dir, mut out) = open_empty("plan-only");
        out.plan("src/deep/a.js").expect("plan");
        assert_eq!(fs::read_dir(&dir).expect("read dir").count(), 0);
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn unpack_output_path_rejects_symlink_parent_that_points_outside() {
        let dir = temp_test_dir("symlink-parent");
        let out_dir = dir.join("out");
        let external_dir = dir.join("external");
        fs::create_dir_all(&out_dir).expect("create output dir");
        fs::create_dir_all(&external_dir).expect("create external dir");
        if create_dir_symlink(&external_dir, &out_dir.join("link")).is_err() {
            fs::remove_dir_all(&dir).expect("remove temp dir");
            return;
        }
        let mut out = OutputDir::open(&out_dir, true).expect("open output dir");

        assert_escapes(out.claim("link/pwn.js"));
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_skips_identical_readonly_file_in_existing_dir() {
        let dir = temp_test_dir("write-if-changed");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("entry.js");
        fs::write(&path, "same").expect("write temp file");
        let original_permissions = fs::metadata(&path).expect("metadata").permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).expect("set readonly");

        let mut out = OutputDir::open(&dir, true).expect("open output dir");
        let file = out.claim("entry.js").expect("claim");
        assert!(out.write(&file, b"same").is_ok());

        fs::set_permissions(&path, original_permissions).expect("restore permissions");
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_overwrites_changed_file_in_existing_dir() {
        let dir = temp_test_dir("write-if-changed-length");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(dir.join("entry.js"), "short").expect("write temp file");

        let mut out = OutputDir::open(&dir, true).expect("open output dir");
        let file = out.claim("entry.js").expect("claim");
        out.write(&file, b"longer content")
            .expect("write changed file");

        assert_eq!(
            fs::read_to_string(dir.join("entry.js")).expect("read updated file"),
            "longer content"
        );
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn write_preserves_non_utf8_and_nul_bytes() {
        let (dir, mut out) = open_empty("write-bytes");
        let file = out.claim("asset.bin").expect("claim");
        let bytes = [0, 0xff, b'B', b'U', b'N', 0];

        out.write(&file, &bytes).expect("write binary asset");

        assert_eq!(fs::read(file.path()).expect("read binary asset"), bytes);
        fs::remove_dir_all(&dir).expect("remove temp dir");
    }
}
