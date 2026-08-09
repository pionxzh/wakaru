//! Byte-exact extraction of Bun standalone file records.
//!
//! This command deliberately stops at the container boundary. JavaScript files
//! are written exactly as Bun stored them, alongside every non-JavaScript file;
//! callers can pass selected JavaScript outputs back through Wakaru separately.

use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal};
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::{
    canonicalize_output_dir, canonicalize_unpack_output_path, resolve_unpack_output_path,
    write_bytes, write_bytes_if_changed,
};

const MANIFEST_SCHEMA: u32 = 1;
const MAX_OUTPUT_AMPLIFICATION: usize = 4;

#[derive(Debug, Clone, Args)]
pub(crate) struct BunArgs {
    #[command(subcommand)]
    pub(crate) command: BunCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum BunCommand {
    /// Extract every file embedded in a Bun single-file executable.
    Extract(BunExtractArgs),
}

#[derive(Debug, Clone, Args)]
pub(crate) struct BunExtractArgs {
    /// Bun single-file executable: PE, Mach-O, ELF, or bare serialized graph.
    pub(crate) input: PathBuf,

    /// Output directory. Embedded files are written below `files/`.
    #[arg(short, long, value_name = "DIR")]
    pub(crate) output: PathBuf,

    /// Also write Bun's opaque source-map, bytecode, and module-info regions.
    #[arg(long)]
    pub(crate) include_internals: bool,

    /// Print the extraction manifest to stdout as JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn run(args: BunArgs, force: bool) -> Result<()> {
    match args.command {
        BunCommand::Extract(args) => run_extract(args, force),
    }
}

fn run_extract(args: BunExtractArgs, force: bool) -> Result<()> {
    let executable = fs::read(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let standalone = wakaru::bun::extract_standalone(&executable)
        .with_context(|| {
            format!(
                "failed to extract Bun single-file executable {}",
                args.input.display()
            )
        })?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} does not contain a supported Bun standalone graph",
                args.input.display()
            )
        })?;

    let requested_bytes = standalone.files.iter().try_fold(0usize, |total, file| {
        let internals = if args.include_internals {
            file.source_map
                .len()
                .checked_add(file.bytecode.len())
                .and_then(|size| size.checked_add(file.module_info.len()))
                .ok_or_else(|| anyhow::anyhow!("Bun extraction size overflows this platform"))?
        } else {
            0
        };
        total
            .checked_add(file.contents.len())
            .and_then(|size| size.checked_add(internals))
            .ok_or_else(|| anyhow::anyhow!("Bun extraction size overflows this platform"))
    })?;
    let output_limit = executable.len().saturating_mul(MAX_OUTPUT_AMPLIFICATION);
    if requested_bytes > output_limit {
        bail!(
            "Bun graph requests {requested_bytes} output bytes from a {}-byte input; refusing suspicious extraction amplification",
            executable.len()
        );
    }

    let check_existing_writes = crate::ensure_output_dir(&args.output, force)?;
    let output_root = canonicalize_output_dir(&args.output)?;
    let mut exact_paths = HashSet::new();
    let mut case_folded_paths = HashSet::new();
    let mut planned = Vec::with_capacity(standalone.files.len());

    for file in &standalone.files {
        let sanitized = sanitize_embedded_path(file.name_bytes, file.index, file.loader_kind());
        let unique = allocate_case_insensitive_path(&sanitized, &mut case_folded_paths);
        let relative_output = format!("files/{unique}");
        let output_path =
            resolve_unpack_output_path(&output_root, &relative_output, &mut exact_paths)?;

        let source_map_output = plan_internal_output(
            &output_root,
            &mut exact_paths,
            args.include_internals,
            file.index,
            "source-map.bunmap",
            file.source_map,
        )?;
        let bytecode_output = plan_internal_output(
            &output_root,
            &mut exact_paths,
            args.include_internals,
            file.index,
            "bytecode.bin",
            file.bytecode,
        )?;
        let module_info_output = plan_internal_output(
            &output_root,
            &mut exact_paths,
            args.include_internals,
            file.index,
            "module-info.bin",
            file.module_info,
        )?;

        planned.push(PlannedFile {
            file,
            relative_output,
            output_path,
            source_map_output,
            bytecode_output,
            module_info_output,
        });
    }

    for file in &planned {
        write_output(&file.output_path, file.file.contents, check_existing_writes)?;
        for (output, bytes) in [
            (&file.source_map_output, file.file.source_map),
            (&file.bytecode_output, file.file.bytecode),
            (&file.module_info_output, file.file.module_info),
        ] {
            if let Some((_, path)) = output {
                write_output(path, bytes, check_existing_writes)?;
            }
        }
    }

    let manifest = BunManifest::new(
        &args.input,
        &standalone,
        &planned,
        args.include_internals,
        requested_bytes,
    );
    let serialized = format!("{}\n", serde_json::to_string_pretty(&manifest)?);
    let manifest_path = canonicalize_unpack_output_path(
        &output_root,
        &output_root.join("manifest.json"),
        "manifest.json",
    )?;
    if check_existing_writes {
        write_bytes_if_changed(&manifest_path, serialized.as_bytes())?;
    } else {
        write_bytes(&manifest_path, serialized.as_bytes())?;
    }

    if args.json {
        print!("{serialized}");
    } else if io::stderr().is_terminal() {
        eprintln!(
            "extracted {} Bun file(s), {} byte(s), to {}",
            planned.len(),
            planned
                .iter()
                .map(|file| file.file.contents.len())
                .sum::<usize>(),
            args.output.display()
        );
    }

    Ok(())
}

fn write_output(path: &Path, bytes: &[u8], check_existing: bool) -> Result<()> {
    if check_existing {
        write_bytes_if_changed(path, bytes)
    } else {
        write_bytes(path, bytes)
    }
}

fn plan_internal_output(
    output_root: &Path,
    seen: &mut HashSet<String>,
    include_internals: bool,
    index: u32,
    filename: &str,
    bytes: &[u8],
) -> Result<Option<(String, PathBuf)>> {
    if !include_internals || bytes.is_empty() {
        return Ok(None);
    }
    let relative = format!("internals/{index:04}/{filename}");
    let output = resolve_unpack_output_path(output_root, &relative, seen)?;
    Ok(Some((relative, output)))
}

struct PlannedFile<'file, 'data> {
    file: &'file wakaru::bun::BunEmbeddedFile<'data>,
    relative_output: String,
    output_path: PathBuf,
    source_map_output: Option<(String, PathBuf)>,
    bytecode_output: Option<(String, PathBuf)>,
    module_info_output: Option<(String, PathBuf)>,
}

fn sanitize_embedded_path(name: &[u8], index: u32, loader: wakaru::bun::BunLoader) -> String {
    let normalized = name
        .iter()
        .map(|byte| if *byte == b'\\' { b'/' } else { *byte })
        .collect::<Vec<_>>();
    let without_prefix = [
        b"/$bunfs/root/".as_slice(),
        b"/$bunfs/".as_slice(),
        b"B:/~BUN/root/".as_slice(),
        b"B:/~BUN/".as_slice(),
    ]
    .into_iter()
    .find_map(|prefix| normalized.strip_prefix(prefix))
    .unwrap_or(&normalized);

    let components = without_prefix
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty() && *component != b".")
        .map(encode_path_component)
        .collect::<Vec<_>>();
    if components.is_empty() {
        format!("embedded-{index}.{}", fallback_extension(loader))
    } else {
        components.join("/")
    }
}

fn encode_path_component(component: &[u8]) -> String {
    if component == b".." {
        return "%2E%2E".to_string();
    }

    let mut encoded = String::with_capacity(component.len());
    for byte in component {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'$' | b'@' | b'+')
        {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }

    // Win32 strips trailing dots and reserves device basenames even when they
    // have an extension. Encode one byte so the path remains deterministic,
    // portable, and distinct from the original spelling.
    let trailing_dots = component
        .iter()
        .rev()
        .take_while(|byte| **byte == b'.')
        .count();
    if trailing_dots > 0 {
        encoded.truncate(encoded.len() - trailing_dots);
        for _ in 0..trailing_dots {
            encoded.push_str("%2E");
        }
    }
    if is_windows_device_name(component) {
        let first = component[0];
        encoded.replace_range(
            ..1,
            &format!("%{}{}", hex_digit(first >> 4), hex_digit(first & 0x0f)),
        );
    }
    encoded
}

fn is_windows_device_name(component: &[u8]) -> bool {
    let stem = component
        .split(|byte| *byte == b'.')
        .next()
        .unwrap_or(component);
    matches_ascii_case_insensitive(stem, b"CON")
        || matches_ascii_case_insensitive(stem, b"PRN")
        || matches_ascii_case_insensitive(stem, b"AUX")
        || matches_ascii_case_insensitive(stem, b"NUL")
        || matches_ascii_case_insensitive(stem, b"CONIN$")
        || matches_ascii_case_insensitive(stem, b"CONOUT$")
        || ((stem.len() == 4)
            && (matches_ascii_case_insensitive(&stem[..3], b"COM")
                || matches_ascii_case_insensitive(&stem[..3], b"LPT"))
            && matches!(stem[3], b'1'..=b'9'))
}

fn matches_ascii_case_insensitive(left: &[u8], right: &[u8]) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'A' + nibble - 10) as char,
        _ => unreachable!("nibble is masked to four bits"),
    }
}

fn fallback_extension(loader: wakaru::bun::BunLoader) -> &'static str {
    use wakaru::bun::BunLoader;

    match loader {
        BunLoader::Jsx => "jsx",
        BunLoader::Js => "js",
        BunLoader::Ts => "ts",
        BunLoader::Tsx => "tsx",
        BunLoader::Css => "css",
        BunLoader::Json => "json",
        BunLoader::Jsonc => "jsonc",
        BunLoader::Toml => "toml",
        BunLoader::Wasm => "wasm",
        BunLoader::Napi => "node",
        BunLoader::Text => "txt",
        BunLoader::BunShell => "sh",
        BunLoader::Sqlite | BunLoader::SqliteEmbedded => "sqlite",
        BunLoader::Html => "html",
        BunLoader::Yaml => "yaml",
        BunLoader::Json5 => "json5",
        BunLoader::Markdown => "md",
        BunLoader::File | BunLoader::Base64 | BunLoader::DataUrl | BunLoader::Unknown(_) => "bin",
        _ => "bin",
    }
}

fn allocate_case_insensitive_path(path: &str, seen: &mut HashSet<String>) -> String {
    if seen.insert(path.to_ascii_lowercase()) {
        return path.to_string();
    }

    let path = Path::new(path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("sanitized path is non-empty ASCII");
    let (stem, extension) = filename
        .rsplit_once('.')
        .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
        .map_or((filename, None), |(stem, extension)| {
            (stem, Some(extension))
        });

    for suffix in 2usize.. {
        let filename = match extension {
            Some(extension) => format!("{stem}_{suffix}.{extension}"),
            None => format!("{stem}_{suffix}"),
        };
        let candidate = if parent.as_os_str().is_empty() {
            filename
        } else {
            format!("{}/{}", parent.display(), filename)
        };
        if seen.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
    }

    unreachable!("an unbounded numeric suffix always has an unused value")
}

#[derive(Serialize)]
struct BunManifest {
    schema: u32,
    input: String,
    entry_point_id: u32,
    flags: u32,
    executable_range: [usize; 2],
    compile_exec_argv: ManifestBytes,
    include_internals: bool,
    file_count: usize,
    javascript_file_count: usize,
    asset_file_count: usize,
    content_bytes: usize,
    written_bytes: usize,
    files: Vec<BunManifestFile>,
}

impl BunManifest {
    fn new(
        input: &Path,
        standalone: &wakaru::bun::BunStandalone<'_>,
        planned: &[PlannedFile<'_, '_>],
        include_internals: bool,
        written_bytes: usize,
    ) -> Self {
        let javascript_file_count = planned
            .iter()
            .filter(|file| file.file.is_javascript_like())
            .count();
        let content_bytes = planned.iter().map(|file| file.file.contents.len()).sum();
        Self {
            schema: MANIFEST_SCHEMA,
            input: input.to_string_lossy().into_owned(),
            entry_point_id: standalone.entry_point_id,
            flags: standalone.flags,
            executable_range: range_array(&standalone.executable_range),
            compile_exec_argv: ManifestBytes::new(standalone.compile_exec_argv),
            include_internals,
            file_count: planned.len(),
            javascript_file_count,
            asset_file_count: planned.len() - javascript_file_count,
            content_bytes,
            written_bytes,
            files: planned.iter().map(BunManifestFile::new).collect(),
        }
    }
}

#[derive(Serialize)]
struct BunManifestFile {
    index: u32,
    original_name: ManifestBytes,
    output_path: String,
    loader: String,
    loader_id: u8,
    encoding: &'static str,
    module_format: &'static str,
    side: &'static str,
    is_entry: bool,
    bytes: usize,
    executable_range: [usize; 2],
    source_map: ManifestRegion,
    bytecode: ManifestRegion,
    module_info: ManifestRegion,
    bytecode_origin_path: ManifestBytes,
}

impl BunManifestFile {
    fn new(planned: &PlannedFile<'_, '_>) -> Self {
        let file = planned.file;
        Self {
            index: file.index,
            original_name: ManifestBytes::new(file.name_bytes),
            output_path: planned.relative_output.clone(),
            loader: file.loader_kind().as_str().to_string(),
            loader_id: file.loader,
            encoding: encoding_name(file.encoding),
            module_format: module_format_name(file.module_format),
            side: side_name(file.side),
            is_entry: file.is_entry,
            bytes: file.contents.len(),
            executable_range: range_array(&file.executable_range),
            source_map: ManifestRegion::new(
                file.source_map.len(),
                file.source_map_range.as_ref(),
                planned.source_map_output.as_ref(),
            ),
            bytecode: ManifestRegion::new(
                file.bytecode.len(),
                file.bytecode_range.as_ref(),
                planned.bytecode_output.as_ref(),
            ),
            module_info: ManifestRegion::new(
                file.module_info.len(),
                file.module_info_range.as_ref(),
                planned.module_info_output.as_ref(),
            ),
            bytecode_origin_path: ManifestBytes::new(file.bytecode_origin_path),
        }
    }
}

#[derive(Serialize)]
struct ManifestRegion {
    bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable_range: Option<[usize; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_path: Option<String>,
}

impl ManifestRegion {
    fn new(bytes: usize, range: Option<&Range<usize>>, output: Option<&(String, PathBuf)>) -> Self {
        Self {
            bytes,
            executable_range: range.map(range_array),
            output_path: output.map(|(relative, _)| relative.clone()),
        }
    }
}

#[derive(Serialize)]
struct ManifestBytes {
    #[serde(skip_serializing_if = "Option::is_none")]
    utf8: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hex: Option<String>,
}

impl ManifestBytes {
    fn new(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self {
                utf8: Some(text.to_string()),
                hex: None,
            },
            Err(_) => Self {
                utf8: None,
                hex: Some(
                    bytes
                        .iter()
                        .flat_map(|byte| [hex_digit(byte >> 4), hex_digit(byte & 0x0f)])
                        .collect(),
                ),
            },
        }
    }
}

fn range_array(range: &Range<usize>) -> [usize; 2] {
    [range.start, range.end]
}

fn encoding_name(encoding: wakaru::bun::BunEncoding) -> &'static str {
    match encoding {
        wakaru::bun::BunEncoding::Binary => "binary",
        wakaru::bun::BunEncoding::Latin1 => "latin1",
        wakaru::bun::BunEncoding::Utf8 => "utf8",
        _ => "unknown",
    }
}

fn module_format_name(format: wakaru::bun::BunModuleFormat) -> &'static str {
    match format {
        wakaru::bun::BunModuleFormat::None => "none",
        wakaru::bun::BunModuleFormat::Esm => "esm",
        wakaru::bun::BunModuleFormat::Cjs => "cjs",
        _ => "unknown",
    }
}

fn side_name(side: wakaru::bun::BunFileSide) -> &'static str {
    match side {
        wakaru::bun::BunFileSide::Server => "server",
        wakaru::bun::BunFileSide::Client => "client",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const RECORD_SIZE: usize = 52;
    const TRAILER: &[u8] = b"\n---- Bun! ----\n";

    #[derive(Clone, Copy)]
    struct Pointer {
        offset: u32,
        length: u32,
    }

    fn append(data: &mut Vec<u8>, bytes: &[u8], nul: bool) -> Pointer {
        let pointer = Pointer {
            offset: data.len() as u32,
            length: bytes.len() as u32,
        };
        data.extend_from_slice(bytes);
        if nul {
            data.push(0);
        }
        pointer
    }

    fn put_pointer(record: &mut [u8], offset: usize, pointer: Pointer) {
        record[offset..offset + 4].copy_from_slice(&pointer.offset.to_le_bytes());
        record[offset + 4..offset + 8].copy_from_slice(&pointer.length.to_le_bytes());
    }

    fn fixture() -> Vec<u8> {
        let mut data = Vec::new();
        let entry_name = append(&mut data, b"/$bunfs/root/src/entry.js", true);
        let entry_contents = append(&mut data, b"console.log('entry');", true);
        let source_map = append(&mut data, b"opaque-map", false);
        let bytecode = append(&mut data, b"\0\xffbytecode", false);
        let module_info = append(&mut data, b"module-info", false);
        let bytecode_origin = append(&mut data, b"/$bunfs/root/entry.bytecode", true);
        let asset_name = append(&mut data, b"/$bunfs/root/../assets/logo-\xff.bin", true);
        let asset_contents = append(&mut data, b"\x89PNG\0\xff", true);
        let collision_name = append(&mut data, b"/$bunfs/root/../assets/LOGO-\xff.bin", true);
        let collision_contents = append(&mut data, b"second", true);
        let argv = append(&mut data, b"--smol", true);

        let modules_offset = data.len() as u32;
        let mut entry = [0u8; RECORD_SIZE];
        put_pointer(&mut entry, 0, entry_name);
        put_pointer(&mut entry, 8, entry_contents);
        put_pointer(&mut entry, 16, source_map);
        put_pointer(&mut entry, 24, bytecode);
        put_pointer(&mut entry, 32, module_info);
        put_pointer(&mut entry, 40, bytecode_origin);
        entry[48] = 1;
        entry[49] = 1;
        entry[50] = 1;
        data.extend_from_slice(&entry);

        let mut asset = [0u8; RECORD_SIZE];
        put_pointer(&mut asset, 0, asset_name);
        put_pointer(&mut asset, 8, asset_contents);
        asset[49] = 5;
        asset[51] = 1;
        data.extend_from_slice(&asset);

        let mut collision = [0u8; RECORD_SIZE];
        put_pointer(&mut collision, 0, collision_name);
        put_pointer(&mut collision, 8, collision_contents);
        collision[49] = 250;
        data.extend_from_slice(&collision);

        let modules = Pointer {
            offset: modules_offset,
            length: (RECORD_SIZE * 3) as u32,
        };
        let mut executable = b"\x7fELFsynthetic".to_vec();
        executable.extend_from_slice(&data);
        executable.extend_from_slice(&(data.len() as u64).to_le_bytes());
        executable.extend_from_slice(&modules.offset.to_le_bytes());
        executable.extend_from_slice(&modules.length.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&argv.offset.to_le_bytes());
        executable.extend_from_slice(&argv.length.to_le_bytes());
        executable.extend_from_slice(&3u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);
        executable
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wakaru-bun-extract-test-{name}-{nanos}"))
    }

    #[test]
    fn sanitizes_virtual_roots_traversal_and_non_utf8_bytes() {
        assert_eq!(
            sanitize_embedded_path(
                b"B:\\~BUN\\root\\assets\\logo.png",
                0,
                wakaru::bun::BunLoader::File,
            ),
            "assets/logo.png"
        );
        assert_eq!(
            sanitize_embedded_path(
                b"/$bunfs/root/../../logo-\xff.bin",
                1,
                wakaru::bun::BunLoader::File,
            ),
            "%2E%2E/%2E%2E/logo-%FF.bin"
        );
        assert_eq!(
            sanitize_embedded_path(b"../", 7, wakaru::bun::BunLoader::Wasm),
            "%2E%2E"
        );
        assert_eq!(
            sanitize_embedded_path(b"", 8, wakaru::bun::BunLoader::Wasm),
            "embedded-8.wasm"
        );
        assert_eq!(
            sanitize_embedded_path(b"/$bunfs/root/CON.txt", 9, wakaru::bun::BunLoader::Text,),
            "%43ON.txt"
        );
        assert_eq!(
            sanitize_embedded_path(b"assets/name...", 10, wakaru::bun::BunLoader::File),
            "assets/name%2E%2E%2E"
        );
    }

    #[test]
    fn allocates_case_insensitive_collisions_deterministically() {
        let mut seen = HashSet::new();
        assert_eq!(
            allocate_case_insensitive_path("assets/logo.png", &mut seen),
            "assets/logo.png"
        );
        assert_eq!(
            allocate_case_insensitive_path("assets/LOGO.png", &mut seen),
            "assets/LOGO_2.png"
        );
        assert_eq!(
            allocate_case_insensitive_path("assets/logo.png", &mut seen),
            "assets/logo_3.png"
        );
    }

    #[test]
    fn extracts_every_file_and_optional_internal_region_byte_exactly() {
        let dir = temp_test_dir("all-files");
        fs::create_dir_all(&dir).expect("create temp root");
        let input = dir.join("app");
        let output = dir.join("out");
        fs::write(&input, fixture()).expect("write fixture");

        run_extract(
            BunExtractArgs {
                input,
                output: output.clone(),
                include_internals: true,
                json: false,
            },
            false,
        )
        .expect("extract fixture");

        assert_eq!(
            fs::read(output.join("files/src/entry.js")).expect("read entry"),
            b"console.log('entry');"
        );
        assert_eq!(
            fs::read(output.join("files/%2E%2E/assets/logo-%FF.bin")).expect("read binary asset"),
            b"\x89PNG\0\xff"
        );
        assert_eq!(
            fs::read(output.join("files/%2E%2E/assets/LOGO-%FF_2.bin")).expect("read collision"),
            b"second"
        );
        assert_eq!(
            fs::read(output.join("internals/0000/source-map.bunmap")).expect("read source map"),
            b"opaque-map"
        );
        assert_eq!(
            fs::read(output.join("internals/0000/bytecode.bin")).expect("read bytecode"),
            b"\0\xffbytecode"
        );
        assert_eq!(
            fs::read(output.join("internals/0000/module-info.bin")).expect("read module info"),
            b"module-info"
        );

        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("manifest.json")).expect("read manifest"))
                .expect("parse manifest");
        assert_eq!(manifest["file_count"], 3);
        assert_eq!(manifest["javascript_file_count"], 1);
        assert_eq!(manifest["asset_file_count"], 2);
        assert_eq!(manifest["files"][2]["loader"], "unknown");
        assert_eq!(manifest["files"][2]["loader_id"], 250);
        assert_eq!(
            manifest["files"][1]["original_name"]["hex"],
            "2F2462756E66732F726F6F742F2E2E2F6173736574732F6C6F676F2DFF2E62696E"
        );

        fs::remove_dir_all(&dir).expect("remove temp root");
    }

    #[test]
    fn extracts_assets_from_both_real_bun_record_layouts() {
        let fixtures: [(&str, &[u8]); 3] = [
            (
                "1.3.3",
                include_bytes!("../tests/fixtures/bun-standalone-assets/standalone-v1.3.3.bin"),
            ),
            (
                "1.3.8",
                include_bytes!("../tests/fixtures/bun-standalone-assets/standalone-v1.3.8.bin"),
            ),
            (
                "1.3.13",
                include_bytes!("../tests/fixtures/bun-standalone-assets/standalone.bin"),
            ),
        ];

        for (version, graph) in fixtures {
            let dir = temp_test_dir(&format!("real-bun-{version}"));
            fs::create_dir_all(&dir).expect("create temp root");
            let input = dir.join("standalone.bin");
            let output = dir.join("out");
            fs::write(&input, graph).expect("write real Bun graph");

            run_extract(
                BunExtractArgs {
                    input,
                    output: output.clone(),
                    include_internals: false,
                    json: false,
                },
                false,
            )
            .unwrap_or_else(|error| panic!("extract Bun {version} graph: {error:#}"));

            let manifest: serde_json::Value = serde_json::from_slice(
                &fs::read(output.join("manifest.json")).expect("read manifest"),
            )
            .expect("parse manifest");
            let files = manifest["files"].as_array().expect("manifest file array");
            let asset = files
                .iter()
                .find(|file| file["loader"] == "file")
                .expect("real graph should contain its file-loader asset");
            let asset_path = asset["output_path"]
                .as_str()
                .expect("asset output path should be text");
            assert_eq!(
                fs::read(output.join(asset_path)).expect("read extracted asset"),
                include_bytes!("../tests/fixtures/bun-standalone-assets/asset.bin"),
                "Bun {version} asset bytes"
            );
            assert_eq!(manifest["file_count"], 2, "Bun {version}");
            assert_eq!(manifest["javascript_file_count"], 1, "Bun {version}");
            assert_eq!(manifest["asset_file_count"], 1, "Bun {version}");

            fs::remove_dir_all(&dir).expect("remove temp root");
        }
    }
}
