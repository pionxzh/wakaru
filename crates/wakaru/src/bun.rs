//! Extraction of Bun standalone module graphs from compiled executables.
//!
//! Bun serializes the same trailer-delimited payload into PE, Mach-O, and ELF
//! executables. Parsing backward from the trailer avoids platform-specific
//! executable-section handling while retaining exact byte provenance.

use std::fmt;
use std::ops::Range;

const TRAILER: &[u8] = b"\n---- Bun! ----\n";
const OFFSETS_SIZE: usize = 32;
const CURRENT_MODULE_RECORD_SIZE: usize = 52;
const LEGACY_MODULE_RECORD_SIZE: usize = 36;

#[derive(Debug, Clone, Copy)]
struct ModuleRecordLayout {
    label: &'static str,
    size: usize,
    module_info_offset: Option<usize>,
    bytecode_origin_offset: Option<usize>,
    metadata_offset: usize,
}

const CURRENT_MODULE_LAYOUT: ModuleRecordLayout = ModuleRecordLayout {
    label: "Bun 1.3.9+ 52-byte",
    size: CURRENT_MODULE_RECORD_SIZE,
    module_info_offset: Some(32),
    bytecode_origin_offset: Some(40),
    metadata_offset: 48,
};

const LEGACY_MODULE_LAYOUT: ModuleRecordLayout = ModuleRecordLayout {
    label: "Bun 1.3.3-1.3.8 36-byte",
    size: LEGACY_MODULE_RECORD_SIZE,
    module_info_offset: None,
    bytecode_origin_offset: None,
    metadata_offset: 32,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StringPointer {
    offset: u32,
    length: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BunEncoding {
    Binary,
    Latin1,
    Utf8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BunModuleFormat {
    None,
    Esm,
    Cjs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BunFileSide {
    Server,
    Client,
}

/// Bun's serialized `Loader` discriminant.
///
/// Bun treats this enum as append-only. [`Unknown`](Self::Unknown) keeps
/// extraction forward-compatible when a newer Bun adds a loader that Wakaru
/// does not know how to name yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BunLoader {
    Jsx,
    Js,
    Ts,
    Tsx,
    Css,
    File,
    Json,
    Jsonc,
    Toml,
    Wasm,
    Napi,
    Base64,
    DataUrl,
    Text,
    BunShell,
    Sqlite,
    SqliteEmbedded,
    Html,
    Yaml,
    Json5,
    Markdown,
    Unknown(u8),
}

impl BunLoader {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Jsx,
            1 => Self::Js,
            2 => Self::Ts,
            3 => Self::Tsx,
            4 => Self::Css,
            5 => Self::File,
            6 => Self::Json,
            7 => Self::Jsonc,
            8 => Self::Toml,
            9 => Self::Wasm,
            10 => Self::Napi,
            11 => Self::Base64,
            12 => Self::DataUrl,
            13 => Self::Text,
            14 => Self::BunShell,
            15 => Self::Sqlite,
            16 => Self::SqliteEmbedded,
            17 => Self::Html,
            18 => Self::Yaml,
            19 => Self::Json5,
            20 => Self::Markdown,
            other => Self::Unknown(other),
        }
    }

    pub fn as_raw(self) -> u8 {
        match self {
            Self::Jsx => 0,
            Self::Js => 1,
            Self::Ts => 2,
            Self::Tsx => 3,
            Self::Css => 4,
            Self::File => 5,
            Self::Json => 6,
            Self::Jsonc => 7,
            Self::Toml => 8,
            Self::Wasm => 9,
            Self::Napi => 10,
            Self::Base64 => 11,
            Self::DataUrl => 12,
            Self::Text => 13,
            Self::BunShell => 14,
            Self::Sqlite => 15,
            Self::SqliteEmbedded => 16,
            Self::Html => 17,
            Self::Yaml => 18,
            Self::Json5 => 19,
            Self::Markdown => 20,
            Self::Unknown(raw) => raw,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jsx => "jsx",
            Self::Js => "js",
            Self::Ts => "ts",
            Self::Tsx => "tsx",
            Self::Css => "css",
            Self::File => "file",
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Toml => "toml",
            Self::Wasm => "wasm",
            Self::Napi => "napi",
            Self::Base64 => "base64",
            Self::DataUrl => "dataurl",
            Self::Text => "text",
            Self::BunShell => "bunsh",
            Self::Sqlite => "sqlite",
            Self::SqliteEmbedded => "sqlite_embedded",
            Self::Html => "html",
            Self::Yaml => "yaml",
            Self::Json5 => "json5",
            Self::Markdown => "md",
            Self::Unknown(_) => "unknown",
        }
    }

    pub fn is_javascript_like(self) -> bool {
        matches!(self, Self::Jsx | Self::Js | Self::Ts | Self::Tsx)
    }
}

/// One file stored in a Bun standalone module graph.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BunEmbeddedFile<'a> {
    pub index: u32,
    /// UTF-8 representation of the embedded path. Invalid UTF-8 is replaced
    /// lossily; use [`name_bytes`](Self::name_bytes) when exact bytes matter.
    pub name: String,
    pub name_bytes: &'a [u8],
    pub contents: &'a [u8],
    /// Bun's internal serialized source-map representation, when present.
    /// This is not a v3 JSON source map.
    pub source_map: &'a [u8],
    pub source_map_range: Option<Range<usize>>,
    /// Opaque JavaScriptCore bytecode region, including Bun's serialized
    /// alignment padding.
    pub bytecode: &'a [u8],
    pub bytecode_range: Option<Range<usize>>,
    /// Opaque metadata associated with ESM bytecode.
    pub module_info: &'a [u8],
    pub module_info_range: Option<Range<usize>>,
    /// File path used by Bun when generating the bytecode cache.
    pub bytecode_origin_path: &'a [u8],
    pub bytecode_origin_path_range: Option<Range<usize>>,
    pub bytecode_size: u32,
    pub module_info_size: u32,
    pub encoding: BunEncoding,
    /// Raw Bun `Loader` discriminant. Values 0 through 3 are JSX, JS, TS,
    /// and TSX respectively.
    pub loader: u8,
    pub module_format: BunModuleFormat,
    pub side: BunFileSide,
    pub is_entry: bool,
    /// Absolute byte range of `contents` in the executable.
    pub executable_range: Range<usize>,
}

impl BunEmbeddedFile<'_> {
    pub fn is_javascript_like(&self) -> bool {
        self.loader_kind().is_javascript_like()
    }

    pub fn loader_kind(&self) -> BunLoader {
        BunLoader::from_raw(self.loader)
    }
}

/// Validated Bun standalone module graph borrowed from an executable.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BunStandalone<'a> {
    pub files: Vec<BunEmbeddedFile<'a>>,
    pub entry_point_id: u32,
    pub compile_exec_argv: &'a [u8],
    pub compile_exec_argv_range: Option<Range<usize>>,
    pub flags: u32,
    /// Absolute range containing the serialized data, offsets, and trailer.
    pub executable_range: Range<usize>,
}

impl BunStandalone<'_> {
    pub fn entry_point(&self) -> &BunEmbeddedFile<'_> {
        &self.files[self.entry_point_id as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BunStandaloneError {
    message: String,
}

impl BunStandaloneError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BunStandaloneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BunStandaloneError {}

/// Extract a Bun standalone graph from PE, Mach-O, ELF, or a bare serialized
/// payload. Returns `Ok(None)` when the Bun trailer is absent.
pub fn extract_standalone(
    executable: &[u8],
) -> Result<Option<BunStandalone<'_>>, BunStandaloneError> {
    let trailer_positions = executable
        .windows(TRAILER.len())
        .enumerate()
        .filter_map(|(index, window)| (window == TRAILER).then_some(index))
        .collect::<Vec<_>>();
    if trailer_positions.is_empty() {
        return Ok(None);
    }

    let mut last_error = None;
    for &trailer_start in trailer_positions.iter().rev() {
        match parse_candidate(executable, trailer_start) {
            Ok(standalone) => return Ok(Some(standalone)),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| BunStandaloneError::new("invalid Bun standalone payload")))
}

fn parse_candidate(
    executable: &[u8],
    trailer_start: usize,
) -> Result<BunStandalone<'_>, BunStandaloneError> {
    let offsets_start = trailer_start
        .checked_sub(OFFSETS_SIZE)
        .ok_or_else(|| BunStandaloneError::new("Bun trailer has no offsets record"))?;
    let offsets = &executable[offsets_start..trailer_start];
    let byte_count = usize::try_from(read_u64(offsets, 0)?)
        .map_err(|_| BunStandaloneError::new("Bun byte count does not fit this platform"))?;
    let modules_ptr = read_pointer(offsets, 8)?;
    let entry_point_id = read_u32(offsets, 16)?;
    let compile_exec_argv_ptr = read_pointer(offsets, 20)?;
    let flags = read_u32(offsets, 28)?;
    let data_start = offsets_start
        .checked_sub(byte_count)
        .ok_or_else(|| BunStandaloneError::new("Bun byte count exceeds executable prefix"))?;
    let data = &executable[data_start..offsets_start];

    let module_bytes = pointer_slice(data, modules_ptr, "module table")?;
    let compile_exec_argv = pointer_slice(data, compile_exec_argv_ptr, "compile argv")?;
    validate_nul_terminated(data, compile_exec_argv_ptr, "compile argv")?;
    let files = parse_module_table(data, data_start, module_bytes, entry_point_id)?;

    Ok(BunStandalone {
        files,
        entry_point_id,
        compile_exec_argv,
        compile_exec_argv_range: absolute_nonempty_range(data_start, compile_exec_argv_ptr),
        flags,
        executable_range: data_start..trailer_start + TRAILER.len(),
    })
}

fn parse_module_table<'a>(
    data: &'a [u8],
    data_start: usize,
    module_bytes: &'a [u8],
    entry_point_id: u32,
) -> Result<Vec<BunEmbeddedFile<'a>>, BunStandaloneError> {
    if module_bytes.is_empty() {
        return Err(BunStandaloneError::new(
            "unsupported empty Bun module table",
        ));
    }

    let mut parsed = None;
    let mut errors = Vec::new();
    let mut attempted_layout = false;
    for layout in [CURRENT_MODULE_LAYOUT, LEGACY_MODULE_LAYOUT] {
        if !module_bytes.len().is_multiple_of(layout.size) {
            continue;
        }
        attempted_layout = true;
        match parse_module_table_with_layout(data, data_start, module_bytes, entry_point_id, layout)
        {
            Ok(_) if parsed.is_some() => {
                return Err(BunStandaloneError::new(format!(
                    "ambiguous Bun module table length {} matches both supported record layouts",
                    module_bytes.len()
                )));
            }
            Ok(files) => parsed = Some(files),
            Err(error) => errors.push(format!("{} records: {error}", layout.label)),
        }
    }

    if let Some(files) = parsed {
        return Ok(files);
    }
    if !attempted_layout {
        return Err(BunStandaloneError::new(format!(
            "unsupported Bun module table length {} (expected 36- or 52-byte records)",
            module_bytes.len()
        )));
    }

    Err(BunStandaloneError::new(format!(
        "invalid Bun module table ({})",
        errors.join("; ")
    )))
}

fn parse_module_table_with_layout<'a>(
    data: &'a [u8],
    data_start: usize,
    module_bytes: &'a [u8],
    entry_point_id: u32,
    layout: ModuleRecordLayout,
) -> Result<Vec<BunEmbeddedFile<'a>>, BunStandaloneError> {
    let module_count = module_bytes.len() / layout.size;
    if entry_point_id as usize >= module_count {
        return Err(BunStandaloneError::new(format!(
            "Bun entry point {entry_point_id} is outside {module_count} module records"
        )));
    }

    let mut files = Vec::with_capacity(module_count);
    for index in 0..module_count {
        let record = &module_bytes[index * layout.size..(index + 1) * layout.size];
        let name_ptr = read_pointer(record, 0)?;
        let contents_ptr = read_pointer(record, 8)?;
        let source_map_ptr = read_pointer(record, 16)?;
        let bytecode_ptr = read_pointer(record, 24)?;
        let module_info_ptr = match layout.module_info_offset {
            Some(offset) => read_pointer(record, offset)?,
            None => StringPointer {
                offset: 0,
                length: 0,
            },
        };
        let bytecode_origin_ptr = match layout.bytecode_origin_offset {
            Some(offset) => read_pointer(record, offset)?,
            None => StringPointer {
                offset: 0,
                length: 0,
            },
        };
        let encoding = match record[layout.metadata_offset] {
            0 => BunEncoding::Binary,
            1 => BunEncoding::Latin1,
            2 => BunEncoding::Utf8,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown encoding {value}"
                )))
            }
        };
        let loader = record[layout.metadata_offset + 1];
        let module_format = match record[layout.metadata_offset + 2] {
            0 => BunModuleFormat::None,
            1 => BunModuleFormat::Esm,
            2 => BunModuleFormat::Cjs,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown module format {value}"
                )))
            }
        };
        let side = match record[layout.metadata_offset + 3] {
            0 => BunFileSide::Server,
            1 => BunFileSide::Client,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown file side {value}"
                )))
            }
        };

        let name_bytes = pointer_slice(data, name_ptr, "module name")?;
        validate_nul_terminated(data, name_ptr, &format!("module {index} name"))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let contents = pointer_slice(data, contents_ptr, "module contents")?;
        validate_nul_terminated(data, contents_ptr, &format!("module {index} contents"))?;
        let source_map = pointer_slice(data, source_map_ptr, "module source map")?;
        let bytecode = pointer_slice(data, bytecode_ptr, "module bytecode")?;
        let module_info = pointer_slice(data, module_info_ptr, "module info")?;
        let bytecode_origin_path =
            pointer_slice(data, bytecode_origin_ptr, "bytecode origin path")?;
        validate_nul_terminated(
            data,
            bytecode_origin_ptr,
            &format!("module {index} bytecode origin path"),
        )?;

        files.push(BunEmbeddedFile {
            index: index as u32,
            name,
            name_bytes,
            contents,
            source_map,
            source_map_range: absolute_nonempty_range(data_start, source_map_ptr),
            bytecode,
            bytecode_range: absolute_nonempty_range(data_start, bytecode_ptr),
            module_info,
            module_info_range: absolute_nonempty_range(data_start, module_info_ptr),
            bytecode_origin_path,
            bytecode_origin_path_range: absolute_nonempty_range(data_start, bytecode_origin_ptr),
            bytecode_size: bytecode_ptr.length,
            module_info_size: module_info_ptr.length,
            encoding,
            loader,
            module_format,
            side,
            is_entry: index == entry_point_id as usize,
            executable_range: absolute_range(data_start, contents_ptr),
        });
    }

    Ok(files)
}

fn validate_nul_terminated(
    data: &[u8],
    pointer: StringPointer,
    label: &str,
) -> Result<(), BunStandaloneError> {
    if pointer.length == 0 {
        return Ok(());
    }
    let terminator = (pointer.offset as usize)
        .checked_add(pointer.length as usize)
        .ok_or_else(|| BunStandaloneError::new(format!("Bun {label} range overflows")))?;
    if data.get(terminator) == Some(&0) {
        Ok(())
    } else {
        Err(BunStandaloneError::new(format!(
            "Bun {label} is not NUL-terminated"
        )))
    }
}

fn absolute_range(data_start: usize, pointer: StringPointer) -> Range<usize> {
    let start = data_start + pointer.offset as usize;
    start..start + pointer.length as usize
}

fn absolute_nonempty_range(data_start: usize, pointer: StringPointer) -> Option<Range<usize>> {
    (pointer.length > 0).then(|| absolute_range(data_start, pointer))
}

fn pointer_slice<'a>(
    data: &'a [u8],
    pointer: StringPointer,
    label: &str,
) -> Result<&'a [u8], BunStandaloneError> {
    let start = pointer.offset as usize;
    let end = start
        .checked_add(pointer.length as usize)
        .ok_or_else(|| BunStandaloneError::new(format!("Bun {label} range overflows")))?;
    data.get(start..end).ok_or_else(|| {
        BunStandaloneError::new(format!(
            "Bun {label} range {start}..{end} exceeds {}-byte payload",
            data.len()
        ))
    })
}

fn read_pointer(bytes: &[u8], offset: usize) -> Result<StringPointer, BunStandaloneError> {
    Ok(StringPointer {
        offset: read_u32(bytes, offset)?,
        length: read_u32(bytes, offset + 4)?,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BunStandaloneError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| BunStandaloneError::new("truncated Bun metadata"))?;
    Ok(u32::from_le_bytes(
        value.try_into().expect("length checked"),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, BunStandaloneError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| BunStandaloneError::new("truncated Bun metadata"))?;
    Ok(u64::from_le_bytes(
        value.try_into().expect("length checked"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct TestPointer {
        offset: u32,
        length: u32,
    }

    fn append(data: &mut Vec<u8>, bytes: &[u8], nul: bool) -> TestPointer {
        let pointer = TestPointer {
            offset: data.len() as u32,
            length: bytes.len() as u32,
        };
        data.extend_from_slice(bytes);
        if nul {
            data.push(0);
        }
        pointer
    }

    fn put_pointer(record: &mut [u8], offset: usize, pointer: TestPointer) {
        record[offset..offset + 4].copy_from_slice(&pointer.offset.to_le_bytes());
        record[offset + 4..offset + 8].copy_from_slice(&pointer.length.to_le_bytes());
    }

    fn fixture() -> Vec<u8> {
        let prefix = b"synthetic executable prefix\xff";
        let mut data = Vec::new();
        let first_name = append(&mut data, b"/$bunfs/root/entry.js", true);
        let first_contents = append(&mut data, b"console.log('entry');", true);
        let first_map = append(&mut data, b"internal-map", false);
        let first_bytecode = append(&mut data, b"bytecode-with-padding", false);
        let first_module_info = append(&mut data, b"module-info", false);
        let first_bytecode_origin = append(&mut data, b"/$bunfs/root/entry.js.bytecode", true);
        let second_name = append(&mut data, b"/$bunfs/root/asset-\xff.txt", true);
        let second_contents = append(&mut data, b"asset", true);
        let argv = append(&mut data, b"--smol", true);

        let modules_offset = data.len() as u32;
        let mut first = [0u8; CURRENT_MODULE_RECORD_SIZE];
        put_pointer(&mut first, 0, first_name);
        put_pointer(&mut first, 8, first_contents);
        put_pointer(&mut first, 16, first_map);
        put_pointer(&mut first, 24, first_bytecode);
        put_pointer(&mut first, 32, first_module_info);
        put_pointer(&mut first, 40, first_bytecode_origin);
        first[48] = 1;
        first[49] = 1;
        first[50] = 1;
        first[51] = 0;
        data.extend_from_slice(&first);

        let mut second = [0u8; CURRENT_MODULE_RECORD_SIZE];
        put_pointer(&mut second, 0, second_name);
        put_pointer(&mut second, 8, second_contents);
        second[48] = 0;
        second[49] = 13;
        second[50] = 0;
        second[51] = 1;
        data.extend_from_slice(&second);

        let byte_count = data.len() as u64;
        let modules = TestPointer {
            offset: modules_offset,
            length: (CURRENT_MODULE_RECORD_SIZE * 2) as u32,
        };
        let mut executable = prefix.to_vec();
        executable.extend_from_slice(&data);
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&modules.offset.to_le_bytes());
        executable.extend_from_slice(&modules.length.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&argv.offset.to_le_bytes());
        executable.extend_from_slice(&argv.length.to_le_bytes());
        executable.extend_from_slice(&3u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);
        executable.extend_from_slice(b"signature suffix");
        executable
    }

    fn legacy_fixture() -> Vec<u8> {
        const LEGACY_RECORD_SIZE: usize = 36;

        let prefix = b"synthetic legacy executable prefix\xff";
        let mut data = Vec::new();
        let entry_name = append(&mut data, b"/$bunfs/root/legacy-entry.js", true);
        let entry_contents = append(&mut data, b"console.log('legacy');", true);
        let entry_map = append(&mut data, b"legacy-map", false);
        let entry_bytecode = append(&mut data, b"legacy-bytecode", false);
        let asset_name = append(&mut data, b"/$bunfs/root/legacy-asset.bin", true);
        let asset_contents = append(&mut data, b"\0\xfflegacy-asset", true);
        let argv = append(&mut data, b"--legacy", true);

        let modules_offset = data.len() as u32;
        let mut entry = [0u8; LEGACY_RECORD_SIZE];
        put_pointer(&mut entry, 0, entry_name);
        put_pointer(&mut entry, 8, entry_contents);
        put_pointer(&mut entry, 16, entry_map);
        put_pointer(&mut entry, 24, entry_bytecode);
        entry[32] = 1;
        entry[33] = 1;
        entry[34] = 1;
        entry[35] = 0;
        data.extend_from_slice(&entry);

        let mut asset = [0u8; LEGACY_RECORD_SIZE];
        put_pointer(&mut asset, 0, asset_name);
        put_pointer(&mut asset, 8, asset_contents);
        asset[32] = 0;
        asset[33] = 5;
        asset[34] = 0;
        asset[35] = 1;
        data.extend_from_slice(&asset);

        let byte_count = data.len() as u64;
        let modules = TestPointer {
            offset: modules_offset,
            length: (LEGACY_RECORD_SIZE * 2) as u32,
        };
        let mut executable = prefix.to_vec();
        executable.extend_from_slice(&data);
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&modules.offset.to_le_bytes());
        executable.extend_from_slice(&modules.length.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&argv.offset.to_le_bytes());
        executable.extend_from_slice(&argv.length.to_le_bytes());
        executable.extend_from_slice(&3u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);
        executable
    }

    fn layout_count_fixture(layout: ModuleRecordLayout, module_count: usize) -> Vec<u8> {
        let mut data = Vec::new();
        let pointers = (0..module_count)
            .map(|index| {
                let name = append(
                    &mut data,
                    format!("/$bunfs/root/module-{index}.js").as_bytes(),
                    true,
                );
                let contents = append(&mut data, format!("console.log({index});").as_bytes(), true);
                (name, contents)
            })
            .collect::<Vec<_>>();

        let modules_offset = data.len() as u32;
        for (name, contents) in pointers {
            let mut record = vec![0u8; layout.size];
            put_pointer(&mut record, 0, name);
            put_pointer(&mut record, 8, contents);
            record[layout.metadata_offset] = 1;
            record[layout.metadata_offset + 1] = 1;
            record[layout.metadata_offset + 2] = 1;
            data.extend_from_slice(&record);
        }

        let byte_count = data.len() as u64;
        let mut executable = data;
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&modules_offset.to_le_bytes());
        executable.extend_from_slice(&((layout.size * module_count) as u32).to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);
        executable
    }

    #[test]
    fn extracts_validated_module_table_from_trailer() {
        let executable = fixture();
        let standalone = extract_standalone(&executable)
            .expect("fixture should parse")
            .expect("fixture should be detected");

        assert_eq!(standalone.files.len(), 2);
        assert_eq!(standalone.entry_point().name, "/$bunfs/root/entry.js");
        assert_eq!(standalone.entry_point().contents, b"console.log('entry');");
        assert_eq!(standalone.entry_point().source_map, b"internal-map");
        assert_eq!(standalone.entry_point().bytecode, b"bytecode-with-padding");
        assert_eq!(standalone.entry_point().module_info, b"module-info");
        assert_eq!(
            standalone.entry_point().bytecode_origin_path,
            b"/$bunfs/root/entry.js.bytecode"
        );
        assert_eq!(standalone.entry_point().loader_kind(), BunLoader::Js);
        assert!(standalone.entry_point().is_javascript_like());
        assert!(!standalone.files[1].is_javascript_like());
        assert_eq!(
            standalone.files[1].name_bytes,
            b"/$bunfs/root/asset-\xff.txt"
        );
        assert!(standalone.files[1].name.contains('\u{fffd}'));
        assert_eq!(standalone.files[1].loader_kind(), BunLoader::Text);
        assert_eq!(standalone.compile_exec_argv, b"--smol");
        assert!(standalone.compile_exec_argv_range.is_some());
        assert_eq!(standalone.flags, 3);
        assert_eq!(
            &executable[standalone.entry_point().executable_range.clone()],
            standalone.entry_point().contents
        );
        assert_eq!(
            &executable[standalone.entry_point().source_map_range.clone().unwrap()],
            standalone.entry_point().source_map
        );
        assert_eq!(
            &executable[standalone.entry_point().bytecode_range.clone().unwrap()],
            standalone.entry_point().bytecode
        );
        assert_eq!(
            &executable[standalone.entry_point().module_info_range.clone().unwrap()],
            standalone.entry_point().module_info
        );
        assert_eq!(
            &executable[standalone
                .entry_point()
                .bytecode_origin_path_range
                .clone()
                .unwrap()],
            standalone.entry_point().bytecode_origin_path
        );
    }

    #[test]
    fn extracts_bun_1_3_3_through_1_3_8_module_records() {
        let executable = legacy_fixture();
        let standalone = extract_standalone(&executable)
            .expect("legacy fixture should parse")
            .expect("legacy fixture should be detected");

        assert_eq!(standalone.files.len(), 2);
        assert_eq!(
            standalone.entry_point().name,
            "/$bunfs/root/legacy-entry.js"
        );
        assert_eq!(standalone.entry_point().contents, b"console.log('legacy');");
        assert_eq!(standalone.entry_point().source_map, b"legacy-map");
        assert_eq!(standalone.entry_point().bytecode, b"legacy-bytecode");
        assert!(standalone.entry_point().module_info.is_empty());
        assert!(standalone.entry_point().module_info_range.is_none());
        assert!(standalone.entry_point().bytecode_origin_path.is_empty());
        assert!(standalone
            .entry_point()
            .bytecode_origin_path_range
            .is_none());
        assert_eq!(standalone.entry_point().loader_kind(), BunLoader::Js);
        assert_eq!(standalone.entry_point().module_format, BunModuleFormat::Esm);
        assert_eq!(standalone.entry_point().side, BunFileSide::Server);
        assert_eq!(standalone.files[1].loader_kind(), BunLoader::File);
        assert_eq!(standalone.files[1].side, BunFileSide::Client);
        assert_eq!(standalone.compile_exec_argv, b"--legacy");
        assert_eq!(standalone.flags, 3);
    }

    #[test]
    fn loader_discriminants_match_buns_append_only_enum() {
        let expected = [
            "jsx",
            "js",
            "ts",
            "tsx",
            "css",
            "file",
            "json",
            "jsonc",
            "toml",
            "wasm",
            "napi",
            "base64",
            "dataurl",
            "text",
            "bunsh",
            "sqlite",
            "sqlite_embedded",
            "html",
            "yaml",
            "json5",
            "md",
        ];
        for (raw, name) in expected.into_iter().enumerate() {
            let loader = BunLoader::from_raw(raw as u8);
            assert_eq!(loader.as_raw(), raw as u8);
            assert_eq!(loader.as_str(), name);
        }

        let unknown = BunLoader::from_raw(255);
        assert_eq!(unknown, BunLoader::Unknown(255));
        assert_eq!(unknown.as_raw(), 255);
        assert_eq!(unknown.as_str(), "unknown");
    }

    #[test]
    fn rejects_out_of_bounds_module_pointer() {
        let mut executable = fixture();
        let trailer = executable
            .windows(TRAILER.len())
            .rposition(|window| window == TRAILER)
            .expect("trailer");
        let offsets = trailer - OFFSETS_SIZE;
        executable[offsets + 8..offsets + 12].copy_from_slice(&u32::MAX.to_le_bytes());

        let error = extract_standalone(&executable).expect_err("corrupt pointer should fail");
        assert!(error.to_string().contains("module table range"));
    }

    #[test]
    fn rejects_out_of_bounds_associated_region() {
        let mut executable = fixture();
        let trailer = executable
            .windows(TRAILER.len())
            .rposition(|window| window == TRAILER)
            .expect("trailer");
        let offsets = trailer - OFFSETS_SIZE;
        let byte_count =
            u64::from_le_bytes(executable[offsets..offsets + 8].try_into().unwrap()) as usize;
        let data_start = offsets - byte_count;
        let modules_offset =
            u32::from_le_bytes(executable[offsets + 8..offsets + 12].try_into().unwrap()) as usize;
        let source_map_pointer = data_start + modules_offset + 16;
        executable[source_map_pointer..source_map_pointer + 4]
            .copy_from_slice(&u32::MAX.to_le_bytes());

        let error =
            extract_standalone(&executable).expect_err("corrupt associated pointer should fail");
        assert!(error.to_string().contains("module source map range"));
    }

    #[test]
    fn rejects_nonempty_unterminated_path() {
        let mut executable = fixture();
        let name = b"/$bunfs/root/entry.js";
        let name_start = executable
            .windows(name.len())
            .position(|window| window == name)
            .expect("entry name");
        executable[name_start + name.len()] = b'!';

        let error = extract_standalone(&executable).expect_err("unterminated path should fail");
        assert!(error
            .to_string()
            .contains("module 0 name is not NUL-terminated"));
    }

    #[test]
    fn rejects_nonempty_unterminated_contents() {
        let mut executable = fixture();
        let contents = b"console.log('entry');";
        let contents_start = executable
            .windows(contents.len())
            .position(|window| window == contents)
            .expect("entry contents");
        executable[contents_start + contents.len()] = b'!';

        let error = extract_standalone(&executable).expect_err("unterminated contents should fail");
        assert!(error
            .to_string()
            .contains("module 0 contents is not NUL-terminated"));
    }

    #[test]
    fn rejects_ambiguous_module_record_layout() {
        let modules = vec![0u8; CURRENT_MODULE_RECORD_SIZE * 9];
        assert_eq!(
            modules.len(),
            LEGACY_MODULE_RECORD_SIZE * 13,
            "fixture must be divisible by both supported record sizes"
        );

        let byte_count = modules.len() as u64;
        let mut executable = modules;
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&(byte_count as u32).to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);

        let error = extract_standalone(&executable).expect_err("ambiguous layout should fail");
        assert!(error
            .to_string()
            .contains("matches both supported record layouts"));
    }

    #[test]
    fn distinguishes_realistic_layouts_with_the_same_table_length() {
        let current_fixture = layout_count_fixture(CURRENT_MODULE_LAYOUT, 9);
        let current = extract_standalone(&current_fixture)
            .expect("current layout should parse")
            .expect("current fixture should be detected");
        assert_eq!(current.files.len(), 9);
        assert_eq!(current.files[8].name, "/$bunfs/root/module-8.js");

        let legacy_fixture = layout_count_fixture(LEGACY_MODULE_LAYOUT, 13);
        let legacy = extract_standalone(&legacy_fixture)
            .expect("legacy layout should parse")
            .expect("legacy fixture should be detected");
        assert_eq!(legacy.files.len(), 13);
        assert_eq!(legacy.files[12].name, "/$bunfs/root/module-12.js");
    }

    #[test]
    fn returns_none_without_bun_trailer() {
        assert!(extract_standalone(b"plain JavaScript").unwrap().is_none());
    }
}
