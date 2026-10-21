//! Extraction of Bun standalone module graphs from compiled executables.
//!
//! Bun serializes the same trailer-delimited payload into PE, Mach-O, and ELF
//! executables. Parsing backward from the trailer avoids platform-specific
//! executable-section handling while retaining exact byte provenance.

use std::borrow::Cow;
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

/// How Bun stored a record's `contents`.
///
/// The discriminant is Bun's serialized `Encoding` byte. Use
/// [`BunEmbeddedFile::file_bytes`] or [`BunEmbeddedFile::text`] to read the
/// contents independently of the stored representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BunEncoding {
    /// Stored bytes are the file's bytes (encoding byte 0). Bun uses this for
    /// assets.
    Binary,
    /// Encoding byte 1, followed by one NUL byte. Bun 1.4.0 and earlier label
    /// JavaScript records this way while storing UTF-8 bytes; Bun 1.4.1 and
    /// later use it only for ASCII-only text.
    Latin1,
    /// Never produced. Bun 1.4.0 and earlier declared encoding byte 2 as
    /// UTF-8 but never wrote it; Bun 1.4.1 reuses the value for
    /// [`Utf16`](Self::Utf16), which is how Wakaru reads it. Kept only so
    /// removing it is not a breaking change in a minor release; it will be
    /// removed in the next major release.
    #[deprecated(
        note = "Bun never wrote encoding byte 2 as UTF-8; it is read as `Utf16`. Removed in the next major release."
    )]
    Utf8,
    /// Encoding byte 2 from Bun 1.4.1: little-endian UTF-16 code units at an
    /// even offset, followed by two NUL bytes. Bun stores server-side
    /// JavaScript and `text`-loader imports this way when their UTF-8 text is
    /// not ASCII-only, and presents them to the program as UTF-8.
    Utf16,
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
    /// Stored contents exactly as Bun serialized them, without the
    /// terminator. For [`BunEncoding::Utf16`] these are UTF-16LE code units;
    /// [`file_bytes`](Self::file_bytes) returns the file's own bytes.
    pub contents: &'a [u8],
    /// Bun's internal serialized source-map representation, when present.
    /// This is not a v3 JSON source map; [`original_sources`](Self::original_sources)
    /// reads the original files it carries.
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

impl<'a> BunEmbeddedFile<'a> {
    pub fn is_javascript_like(&self) -> bool {
        self.loader_kind().is_javascript_like()
    }

    pub fn loader_kind(&self) -> BunLoader {
        BunLoader::from_raw(self.loader)
    }

    /// The file's bytes as the bundler emitted them.
    ///
    /// For [`BunEncoding::Binary`] and [`BunEncoding::Latin1`] records this
    /// borrows [`contents`](Self::contents) unchanged. A
    /// [`BunEncoding::Utf16`] record is transcoded back to the WTF-8 bytes
    /// Bun encoded it from: plain UTF-8, except that an unpaired surrogate
    /// keeps its three-byte generalized UTF-8 form instead of being replaced.
    /// The transcode is the exact inverse of Bun's encoding step, so it never
    /// fails.
    pub fn file_bytes(&self) -> Cow<'a, [u8]> {
        match self.encoding {
            BunEncoding::Utf16 => Cow::Owned(utf16le_to_wtf8(self.contents)),
            _ => Cow::Borrowed(self.contents),
        }
    }

    /// The original source files carried in this record's embedded source
    /// map, in the map's `sources` order.
    ///
    /// Bun embeds a map when the build passes `--sourcemap`; without one the
    /// list is empty. [`extract_standalone`] does not read the map, so a
    /// graph whose map this version cannot read still extracts and the error
    /// comes from this call only. Every pointer is checked against the map's own bounds, and each
    /// [`BunOriginalSource`] borrows the executable. Use
    /// [`BunOriginalSource::decompress`] to read a file's contents.
    pub fn original_sources(&self) -> Result<Vec<BunOriginalSource<'a>>, BunStandaloneError> {
        match &self.source_map_range {
            Some(range) => parse_original_sources(self.index, self.source_map, range.start),
            None => Ok(Vec::new()),
        }
    }

    /// The file's contents as text.
    ///
    /// [`BunEncoding::Binary`] and [`BunEncoding::Latin1`] contents must be
    /// valid UTF-8 and are borrowed. Bun 1.4.0 and earlier store UTF-8
    /// JavaScript under the `Latin1` label, and Bun 1.4.1 and later use that
    /// label only for ASCII, so both decode as UTF-8. [`BunEncoding::Utf16`]
    /// contents are decoded as UTF-16LE into an owned string.
    ///
    /// Returns an error instead of replacing characters when the contents are
    /// not valid UTF-8, or when UTF-16 contents hold an unpaired surrogate.
    pub fn text(&self) -> Result<Cow<'a, str>, BunStandaloneError> {
        match self.file_bytes() {
            Cow::Borrowed(bytes) => {
                std::str::from_utf8(bytes)
                    .map(Cow::Borrowed)
                    .map_err(|error| {
                        BunStandaloneError::new(format!(
                            "Bun module {} contents are not UTF-8: {error}",
                            self.index
                        ))
                    })
            }
            Cow::Owned(bytes) => String::from_utf8(bytes).map(Cow::Owned).map_err(|_| {
                BunStandaloneError::new(format!(
                    "Bun module {} UTF-16 contents hold an unpaired surrogate",
                    self.index
                ))
            }),
        }
    }
}

/// One original source file carried in a record's embedded Bun source map.
///
/// Bun copies each `sources` path and `sourcesContent` text of the bundler's
/// JSON source map into its serialized map, compressing each text as one zstd
/// frame. The path is as the bundler wrote it, usually relative to the
/// build's working directory, and may contain `..` segments.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BunOriginalSource<'a> {
    /// Index of the [`BunEmbeddedFile`] whose source map carries this file.
    pub record: u32,
    /// Position of this file in the source map's `sources` list.
    pub index: u32,
    pub path: &'a str,
    /// Absolute byte range of `path` in the executable.
    pub path_range: Range<usize>,
    /// The zstd frame holding the file's contents.
    pub compressed: &'a [u8],
    /// Absolute byte range of `compressed` in the executable.
    pub compressed_range: Range<usize>,
}

impl BunOriginalSource<'_> {
    /// Decompresses the file's contents, as the bundler read them.
    ///
    /// `limit` caps the decompressed size: a frame that declares a larger
    /// content size fails before decoding, and one that decodes past `limit`
    /// fails within one decoding step (64 KiB plus at most one block) of
    /// passing it. The input must be exactly one zstd frame; a
    /// declared content size and a frame checksum, when present, must match
    /// the decoded bytes. Returns an error instead of a truncated file.
    pub fn decompress(&self, limit: usize) -> Result<Vec<u8>, BunStandaloneError> {
        use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

        let error = |message: String| {
            BunStandaloneError::new(format!(
                "Bun module {} source {} ({}): {message}",
                self.record, self.index, self.path
            ))
        };
        // The frame header descriptor follows the four-byte magic. It
        // declares the content size when its size flag is set or when the
        // frame is a single segment.
        let declares_size = self
            .compressed
            .get(4)
            .is_some_and(|descriptor| descriptor >> 6 != 0 || descriptor & 0x20 != 0);

        let mut input = self.compressed;
        let mut decoder = FrameDecoder::new();
        decoder
            .init(&mut input)
            .map_err(|source| error(format!("invalid zstd frame header: {source}")))?;
        let declared = declares_size.then(|| decoder.content_size());
        if let Some(size) = declared {
            if size > limit as u64 {
                return Err(error(format!(
                    "zstd frame declares {size} bytes, over the {limit}-byte budget"
                )));
            }
        }

        let mut output = Vec::with_capacity(declared.map_or(0, |size| size as usize));
        loop {
            decoder
                .decode_blocks(&mut input, BlockDecodingStrategy::UptoBytes(64 * 1024))
                .map_err(|source| error(format!("invalid zstd frame: {source}")))?;
            if let Some(bytes) = decoder.collect() {
                output.extend_from_slice(&bytes);
            }
            if output.len() > limit {
                return Err(error(format!(
                    "zstd frame decodes past the {limit}-byte budget"
                )));
            }
            if decoder.is_finished() {
                break;
            }
        }
        if let Some(bytes) = decoder.collect() {
            output.extend_from_slice(&bytes);
        }
        if output.len() > limit {
            return Err(error(format!(
                "zstd frame decodes past the {limit}-byte budget"
            )));
        }

        if !input.is_empty() {
            return Err(error(format!(
                "{} bytes follow the zstd frame",
                input.len()
            )));
        }
        if let Some(size) = declared {
            if output.len() as u64 != size {
                return Err(error(format!(
                    "zstd frame declares {size} bytes but decodes to {}",
                    output.len()
                )));
            }
        }
        if let Some(expected) = decoder.get_checksum_from_data() {
            if decoder.get_calculated_checksum() != Some(expected) {
                return Err(error("zstd frame checksum does not match".to_string()));
            }
        }
        Ok(output)
    }
}

/// Size of `SerializedSourceMapHeader`: `source_files_count` and
/// `map_bytes_length`, both `u32`.
const SOURCE_MAP_HEADER_SIZE: usize = 8;
const SOURCE_MAP_POINTER_SIZE: usize = 8;

/// Reads the source paths and compressed contents of Bun's
/// `SerializedSourceMap`, whose layout is stable since at least Bun 1.2:
///
/// ```text
/// u32 source_files_count, u32 map_bytes_length
/// source_files_count × StringPointer   source paths
/// source_files_count × StringPointer   zstd-compressed source contents
/// map_bytes_length bytes               mappings (opaque here)
/// string payload                       paths, then compressed contents
/// ```
///
/// Every pointer is relative to the start of the blob. Bun's writer places
/// the string payload right after the mappings and the first path at its
/// start; both are required here, and every pointer must stay inside the
/// payload. The mapping bytes changed representation across Bun versions and
/// are not read.
fn parse_original_sources(
    record: u32,
    blob: &[u8],
    blob_start: usize,
) -> Result<Vec<BunOriginalSource<'_>>, BunStandaloneError> {
    let error = |message: String| {
        BunStandaloneError::new(format!("Bun module {record} source map {message}"))
    };
    if blob.len() < SOURCE_MAP_HEADER_SIZE {
        return Err(error(format!("header is truncated ({} bytes)", blob.len())));
    }
    let count = read_u32(blob, 0)? as usize;
    let map_bytes_length = read_u32(blob, 4)? as usize;
    let table_end = count
        .checked_mul(2 * SOURCE_MAP_POINTER_SIZE)
        .and_then(|size| size.checked_add(SOURCE_MAP_HEADER_SIZE))
        .filter(|end| *end <= blob.len())
        .ok_or_else(|| {
            error(format!(
                "declares {count} sources, more than its {}-byte blob can index",
                blob.len()
            ))
        })?;
    let payload_start = table_end
        .checked_add(map_bytes_length)
        .filter(|end| *end <= blob.len())
        .ok_or_else(|| {
            error(format!(
                "mapping length {map_bytes_length} runs past its {}-byte blob",
                blob.len()
            ))
        })?;
    if count == 0 {
        return Ok(Vec::new());
    }

    let pointer = |slot: usize| {
        read_pointer(
            blob,
            SOURCE_MAP_HEADER_SIZE + slot * SOURCE_MAP_POINTER_SIZE,
        )
    };
    let first_path = pointer(0)?;
    if first_path.offset as usize != payload_start {
        return Err(error(format!(
            "first source path starts at {}, not at the string payload start {payload_start}",
            first_path.offset
        )));
    }
    let payload_slice = |pointer: StringPointer, what: &str, index: usize| {
        let start = pointer.offset as usize;
        start
            .checked_add(pointer.length as usize)
            .filter(|end| start >= payload_start && *end <= blob.len())
            .map(|end| (&blob[start..end], blob_start + start..blob_start + end))
            .ok_or_else(|| {
                error(format!(
                    "source {index} {what} range {start}+{} lies outside the string payload {payload_start}..{}",
                    pointer.length,
                    blob.len()
                ))
            })
    };

    let mut sources = Vec::with_capacity(count);
    for index in 0..count {
        let (path, path_range) = payload_slice(pointer(index)?, "path", index)?;
        let path = std::str::from_utf8(path)
            .map_err(|_| error(format!("source {index} path is not UTF-8")))?;
        let (compressed, compressed_range) =
            payload_slice(pointer(count + index)?, "contents", index)?;
        sources.push(BunOriginalSource {
            record,
            index: index as u32,
            path,
            path_range,
            compressed,
            compressed_range,
        });
    }
    Ok(sources)
}

/// Transcodes UTF-16LE code units to WTF-8: UTF-8, plus the three-byte form
/// of each unpaired surrogate. `bytes` has an even length (validated at parse
/// time); a trailing odd byte would be ignored.
fn utf16le_to_wtf8(bytes: &[u8]) -> Vec<u8> {
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let mut output = Vec::with_capacity(bytes.len());
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(character) => {
                let mut buffer = [0u8; 4];
                output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            Err(error) => {
                let surrogate = error.unpaired_surrogate();
                output.extend_from_slice(&[
                    0xe0 | (surrogate >> 12) as u8,
                    0x80 | ((surrogate >> 6) & 0x3f) as u8,
                    0x80 | (surrogate & 0x3f) as u8,
                ]);
            }
        }
    }
    output
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
///
/// A present but invalid graph is an error rather than a partial extraction.
/// All returned byte slices borrow the caller-owned executable, and every
/// data pointer in the record (four in Bun 1.3.3–1.3.8, six from 1.3.9) is
/// range-checked before a result is returned; the two newer metadata regions
/// are returned empty for an older record.
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
            2 => BunEncoding::Utf16,
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
        let contents_label = format!("module {index} contents");
        if encoding == BunEncoding::Utf16 {
            validate_utf16_body(data, contents_ptr, &contents_label)?;
        } else {
            validate_nul_terminated(data, contents_ptr, &contents_label)?;
        }
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
    validate_terminator(data, pointer, label, 1)
}

/// Bun 1.4.1+ writes a UTF-16 body at an even offset of the graph data, with
/// an even byte length and a two-byte NUL terminator. The offset is checked
/// relative to the graph data rather than the executable, so a bare graph cut
/// out of an executable validates the same way.
fn validate_utf16_body(
    data: &[u8],
    pointer: StringPointer,
    label: &str,
) -> Result<(), BunStandaloneError> {
    if pointer.length == 0 {
        return Ok(());
    }
    if !pointer.offset.is_multiple_of(2) {
        return Err(BunStandaloneError::new(format!(
            "Bun {label} is UTF-16 at odd offset {}",
            pointer.offset
        )));
    }
    if !pointer.length.is_multiple_of(2) {
        return Err(BunStandaloneError::new(format!(
            "Bun {label} is UTF-16 with odd byte length {}",
            pointer.length
        )));
    }
    validate_terminator(data, pointer, label, 2)
}

/// Checks that `width` NUL bytes follow a nonempty region.
fn validate_terminator(
    data: &[u8],
    pointer: StringPointer,
    label: &str,
    width: usize,
) -> Result<(), BunStandaloneError> {
    if pointer.length == 0 {
        return Ok(());
    }
    let terminator = (pointer.offset as usize)
        .checked_add(pointer.length as usize)
        .and_then(|start| Some(start..start.checked_add(width)?))
        .ok_or_else(|| BunStandaloneError::new(format!("Bun {label} range overflows")))?;
    if data
        .get(terminator)
        .is_some_and(|bytes| bytes.iter().all(|byte| *byte == 0))
    {
        Ok(())
    } else if width == 1 {
        Err(BunStandaloneError::new(format!(
            "Bun {label} is not NUL-terminated"
        )))
    } else {
        Err(BunStandaloneError::new(format!(
            "Bun {label} is not terminated by {width} NUL bytes"
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

    /// Wraps graph `data` holding one 52-byte record in the offsets record and
    /// trailer. `metadata` is the record's encoding, loader, format, and side.
    fn single_record_graph(
        mut data: Vec<u8>,
        name: TestPointer,
        contents: TestPointer,
        source_map: TestPointer,
        metadata: [u8; 4],
    ) -> Vec<u8> {
        let modules_offset = data.len() as u32;
        let mut record = [0u8; CURRENT_MODULE_RECORD_SIZE];
        put_pointer(&mut record, 0, name);
        put_pointer(&mut record, 8, contents);
        put_pointer(&mut record, 16, source_map);
        record[48..52].copy_from_slice(&metadata);
        data.extend_from_slice(&record);

        let byte_count = data.len() as u64;
        let mut executable = data;
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&modules_offset.to_le_bytes());
        executable.extend_from_slice(&(CURRENT_MODULE_RECORD_SIZE as u32).to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(&0u32.to_le_bytes());
        executable.extend_from_slice(TRAILER);
        executable
    }

    #[derive(Clone, Copy, Default)]
    struct Utf16Layout {
        odd_offset: bool,
        odd_length: bool,
        one_nul: bool,
    }

    /// A graph whose JavaScript entry is stored the way Bun 1.4.1+ stores
    /// non-ASCII text: UTF-16LE at an even offset, followed by two NULs.
    fn utf16_fixture(units: &[u16], layout: Utf16Layout) -> Vec<u8> {
        let mut data = Vec::new();
        let name = append(&mut data, b"/$bunfs/root/entry.js", true);
        if data.len().is_multiple_of(2) == layout.odd_offset {
            data.push(0);
        }
        let body = units
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>();
        let contents = TestPointer {
            offset: data.len() as u32,
            length: body.len() as u32 - u32::from(layout.odd_length),
        };
        data.extend_from_slice(&body);
        data.push(0);
        if !layout.one_nul {
            data.push(0);
        }
        // A nonzero byte after the terminator, so a missing NUL is observable.
        data.extend_from_slice(b"--");
        single_record_graph(
            data,
            name,
            contents,
            TestPointer {
                offset: 0,
                length: 0,
            },
            [2, 1, 1, 0],
        )
    }

    #[test]
    fn decodes_utf16_contents_written_by_newer_bun() {
        let text = "/*! \u{a9} legal */console.log('h\u{e9}llo \u{1f600}');";
        let units = text.encode_utf16().collect::<Vec<_>>();
        let executable = utf16_fixture(&units, Utf16Layout::default());
        let standalone = extract_standalone(&executable)
            .expect("UTF-16 fixture should parse")
            .expect("UTF-16 fixture should be detected");

        let entry = standalone.entry_point();
        assert_eq!(entry.encoding, BunEncoding::Utf16);
        assert_eq!(entry.contents.len(), units.len() * 2);
        assert_eq!(
            &executable[entry.executable_range.clone()],
            entry.contents,
            "the executable range covers the stored UTF-16 bytes"
        );
        assert_eq!(entry.file_bytes().as_ref(), text.as_bytes());
        assert_eq!(entry.text().expect("valid UTF-16"), text);
    }

    #[test]
    fn keeps_unpaired_utf16_surrogates_as_wtf8_bytes_but_rejects_them_as_text() {
        let units = [u16::from(b'a'), 0xd800, u16::from(b'b')];
        let executable = utf16_fixture(&units, Utf16Layout::default());
        let standalone = extract_standalone(&executable)
            .expect("UTF-16 fixture should parse")
            .expect("UTF-16 fixture should be detected");

        let entry = standalone.entry_point();
        assert_eq!(entry.file_bytes().as_ref(), b"a\xed\xa0\x80b");
        let error = entry.text().expect_err("an unpaired surrogate is not text");
        assert!(
            error.to_string().contains("unpaired surrogate"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn decodes_latin1_labeled_contents_as_borrowed_utf8() {
        let executable = fixture();
        let standalone = extract_standalone(&executable)
            .expect("fixture should parse")
            .expect("fixture should be detected");

        let entry = standalone.entry_point();
        assert_eq!(entry.encoding, BunEncoding::Latin1);
        assert!(matches!(entry.file_bytes(), Cow::Borrowed(_)));
        assert!(matches!(
            entry.text().expect("UTF-8 contents"),
            Cow::Borrowed("console.log('entry');")
        ));
    }

    #[test]
    fn rejects_malformed_utf16_bodies() {
        let units = "h\u{e9}llo".encode_utf16().collect::<Vec<_>>();
        for (layout, expected) in [
            (
                Utf16Layout {
                    odd_offset: true,
                    ..Utf16Layout::default()
                },
                "module 0 contents is UTF-16 at odd offset",
            ),
            (
                Utf16Layout {
                    odd_length: true,
                    ..Utf16Layout::default()
                },
                "module 0 contents is UTF-16 with odd byte length",
            ),
            (
                Utf16Layout {
                    one_nul: true,
                    ..Utf16Layout::default()
                },
                "module 0 contents is not terminated by 2 NUL bytes",
            ),
        ] {
            let error = extract_standalone(&utf16_fixture(&units, layout))
                .expect_err("malformed UTF-16 body should fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}, got {error}"
            );
        }
    }

    #[test]
    fn rejects_utf16_terminator_past_the_graph_data() {
        let mut data = Vec::new();
        let name = append(&mut data, b"/$bunfs/root/entry.js", true);
        let modules_offset = data.len();
        data.extend_from_slice(&[0u8; CURRENT_MODULE_RECORD_SIZE]);
        if !data.len().is_multiple_of(2) {
            data.push(0);
        }
        // The body ends the graph data with one NUL; the second NUL would be
        // the offsets record that follows.
        let contents = TestPointer {
            offset: data.len() as u32,
            length: 2,
        };
        data.extend_from_slice(&[0xe9, 0x00, 0x00]);
        let record = &mut data[modules_offset..modules_offset + CURRENT_MODULE_RECORD_SIZE];
        put_pointer(record, 0, name);
        put_pointer(record, 8, contents);
        record[48..52].copy_from_slice(&[2, 1, 1, 0]);

        let byte_count = data.len() as u64;
        let mut executable = data;
        executable.extend_from_slice(&byte_count.to_le_bytes());
        executable.extend_from_slice(&(modules_offset as u32).to_le_bytes());
        executable.extend_from_slice(&(CURRENT_MODULE_RECORD_SIZE as u32).to_le_bytes());
        executable.extend_from_slice(&[0u8; 16]);
        executable.extend_from_slice(TRAILER);

        let error = extract_standalone(&executable).expect_err("unterminated UTF-16 should fail");
        assert!(
            error.to_string().contains("not terminated by 2 NUL bytes"),
            "unexpected error: {error}"
        );
    }

    /// A zstd frame holding `content` in one raw block. `declare` writes a
    /// single-segment header with a one-byte content size; otherwise the
    /// header declares no size, only a 1 KiB window.
    fn raw_zstd_frame(content: &[u8], declare: Option<u8>) -> Vec<u8> {
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd];
        match declare {
            Some(size) => frame.extend_from_slice(&[0x20, size]),
            None => frame.extend_from_slice(&[0x00, 0x00]),
        }
        let block_header = ((content.len() as u32) << 3) | 1;
        frame.extend_from_slice(&block_header.to_le_bytes()[..3]);
        frame.extend_from_slice(content);
        frame
    }

    /// Bun's `SerializedSourceMap` layout: header, path pointers, content
    /// pointers, the opaque mapping bytes, then paths and compressed contents.
    fn serialized_source_map(sources: &[(&[u8], Vec<u8>)], mapping: &[u8]) -> Vec<u8> {
        let payload_start = 8 + 16 * sources.len() + mapping.len();
        let mut header = Vec::new();
        header.extend_from_slice(&(sources.len() as u32).to_le_bytes());
        header.extend_from_slice(&(mapping.len() as u32).to_le_bytes());
        let mut payload = Vec::new();
        let mut pointers = Vec::new();
        for bytes in sources
            .iter()
            .map(|(path, _)| *path)
            .chain(sources.iter().map(|(_, compressed)| compressed.as_slice()))
        {
            pointers.push(((payload_start + payload.len()) as u32, bytes.len() as u32));
            payload.extend_from_slice(bytes);
        }
        for (offset, length) in pointers {
            header.extend_from_slice(&offset.to_le_bytes());
            header.extend_from_slice(&length.to_le_bytes());
        }
        header.extend_from_slice(mapping);
        header.extend_from_slice(&payload);
        header
    }

    /// A graph whose JavaScript entry carries `blob` as its source map.
    fn source_map_fixture(blob: &[u8]) -> Vec<u8> {
        let mut data = b"synthetic prefix".to_vec();
        let source_map = append(&mut data, blob, false);
        let name = append(&mut data, b"/$bunfs/root/entry.js", true);
        let contents = append(&mut data, b"console.log(1);", true);
        single_record_graph(data, name, contents, source_map, [1, 1, 1, 0])
    }

    fn original_sources_error(blob: &[u8]) -> String {
        let executable = source_map_fixture(blob);
        let standalone = extract_standalone(&executable)
            .expect("the graph itself stays valid")
            .expect("graph should be detected");
        standalone
            .entry_point()
            .original_sources()
            .expect_err("source map should be rejected")
            .to_string()
    }

    #[test]
    fn reads_original_sources_from_serialized_source_map() {
        let blob = serialized_source_map(
            &[
                (b"src/index.ts", raw_zstd_frame(b"export {};\n", Some(11))),
                (b"../lib/a.ts", raw_zstd_frame(b"// a\n", None)),
            ],
            b"opaque mapping bytes",
        );
        let executable = source_map_fixture(&blob);
        let standalone = extract_standalone(&executable)
            .expect("fixture should parse")
            .expect("fixture should be detected");

        let sources = standalone
            .entry_point()
            .original_sources()
            .expect("valid source map");
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].path, "src/index.ts");
        assert_eq!(sources[1].path, "../lib/a.ts");
        assert_eq!((sources[1].record, sources[1].index), (0, 1));
        for source in &sources {
            assert_eq!(
                &executable[source.path_range.clone()],
                source.path.as_bytes()
            );
            assert_eq!(
                &executable[source.compressed_range.clone()],
                source.compressed
            );
        }
        assert_eq!(sources[0].decompress(64).unwrap(), b"export {};\n");
        assert_eq!(sources[1].decompress(64).unwrap(), b"// a\n");
    }

    #[test]
    fn reads_no_original_sources_without_a_map_or_with_an_empty_one() {
        let executable = fixture();
        let standalone = extract_standalone(&executable).unwrap().unwrap();
        assert!(standalone.files[1].original_sources().unwrap().is_empty());

        let blob = serialized_source_map(&[], b"mapping");
        let executable = source_map_fixture(&blob);
        let standalone = extract_standalone(&executable).unwrap().unwrap();
        assert!(standalone
            .entry_point()
            .original_sources()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_malformed_source_map_headers() {
        let valid =
            serialized_source_map(&[(b"src/a.ts", raw_zstd_frame(b"a", Some(1)))], b"mapping");

        assert!(original_sources_error(&valid[..5]).contains("header is truncated"));

        let mut too_many = valid.clone();
        too_many[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(original_sources_error(&too_many).contains("more than its"));

        let mut long_mapping = valid.clone();
        long_mapping[4..8].copy_from_slice(&(valid.len() as u32).to_le_bytes());
        assert!(original_sources_error(&long_mapping).contains("runs past"));

        // Bun's writer starts the string payload right after the mapping
        // bytes, with the first path there.
        let mut shifted_anchor = valid.clone();
        shifted_anchor[8..12].copy_from_slice(&(8 + 16 + 7 + 1u32).to_le_bytes());
        assert!(original_sources_error(&shifted_anchor).contains("not at the string payload start"));
    }

    #[test]
    fn rejects_source_map_pointers_outside_the_string_payload() {
        let valid =
            serialized_source_map(&[(b"src/a.ts", raw_zstd_frame(b"a", Some(1)))], b"mapping");

        let mut past_end = valid.clone();
        past_end[20..24].copy_from_slice(&(valid.len() as u32).to_le_bytes());
        let error = original_sources_error(&past_end);
        assert!(error.contains("source 0 contents range"), "{error}");
        assert!(error.contains("lies outside the string payload"), "{error}");

        // Contents pointing back into the mapping bytes.
        let mut into_mapping = valid.clone();
        into_mapping[16..20].copy_from_slice(&24u32.to_le_bytes());
        assert!(original_sources_error(&into_mapping).contains("lies outside the string payload"));
    }

    #[test]
    fn rejects_non_utf8_source_paths() {
        let blob = serialized_source_map(&[(b"src/\xff.ts", raw_zstd_frame(b"a", Some(1)))], b"");
        assert!(original_sources_error(&blob).contains("source 0 path is not UTF-8"));
    }

    fn single_source(frame: Vec<u8>) -> Vec<u8> {
        serialized_source_map(&[(b"src/a.ts", frame)], b"")
    }

    fn decompress_single(blob: &[u8], limit: usize) -> Result<Vec<u8>, BunStandaloneError> {
        let executable = source_map_fixture(blob);
        let standalone = extract_standalone(&executable).unwrap().unwrap();
        let sources = standalone.entry_point().original_sources().unwrap();
        sources[0].decompress(limit)
    }

    #[test]
    fn decompression_stops_at_the_byte_budget() {
        let content = vec![b'x'; 200];

        let declared = single_source(raw_zstd_frame(&content, Some(200)));
        let error = decompress_single(&declared, 199).expect_err("declared size over budget");
        assert!(error
            .to_string()
            .contains("declares 200 bytes, over the 199-byte budget"));
        assert_eq!(decompress_single(&declared, 200).unwrap(), content);

        let undeclared = single_source(raw_zstd_frame(&content, None));
        let error = decompress_single(&undeclared, 199).expect_err("decoded size over budget");
        assert!(error
            .to_string()
            .contains("decodes past the 199-byte budget"));
        assert_eq!(decompress_single(&undeclared, 200).unwrap(), content);
    }

    #[test]
    fn decompression_rejects_frames_that_do_not_match_their_slice() {
        let mut trailing = raw_zstd_frame(b"abc", Some(3));
        trailing.push(0);
        let error = decompress_single(&single_source(trailing), 64).expect_err("trailing byte");
        assert!(error.to_string().contains("1 bytes follow the zstd frame"));

        let wrong_size = raw_zstd_frame(b"abc", Some(4));
        assert!(decompress_single(&single_source(wrong_size), 64).is_err());

        let error =
            decompress_single(&single_source(b"not zstd".to_vec()), 64).expect_err("bad magic");
        assert!(error.to_string().contains("invalid zstd frame header"));
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
