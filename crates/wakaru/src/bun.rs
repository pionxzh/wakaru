//! Extraction of Bun standalone module graphs from compiled executables.
//!
//! Bun serializes the same trailer-delimited payload into PE, Mach-O, and ELF
//! executables. Parsing backward from the trailer avoids platform-specific
//! executable-section handling while retaining exact byte provenance.

use std::fmt;
use std::ops::Range;

const TRAILER: &[u8] = b"\n---- Bun! ----\n";
const OFFSETS_SIZE: usize = 32;
const MODULE_RECORD_SIZE: usize = 52;

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

/// One file stored in a Bun standalone module graph.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BunEmbeddedFile<'a> {
    pub index: u32,
    pub name: String,
    pub contents: &'a [u8],
    /// Bun's internal serialized source-map representation, when present.
    /// This is not a v3 JSON source map.
    pub source_map: &'a [u8],
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
        self.loader <= 3
    }
}

/// Validated Bun standalone module graph borrowed from an executable.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BunStandalone<'a> {
    pub files: Vec<BunEmbeddedFile<'a>>,
    pub entry_point_id: u32,
    pub compile_exec_argv: &'a [u8],
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
    if module_bytes.is_empty() || module_bytes.len() % MODULE_RECORD_SIZE != 0 {
        return Err(BunStandaloneError::new(format!(
            "unsupported Bun module table length {} (expected 52-byte records)",
            module_bytes.len()
        )));
    }
    let module_count = module_bytes.len() / MODULE_RECORD_SIZE;
    if entry_point_id as usize >= module_count {
        return Err(BunStandaloneError::new(format!(
            "Bun entry point {entry_point_id} is outside {module_count} module records"
        )));
    }
    let compile_exec_argv = pointer_slice(data, compile_exec_argv_ptr, "compile argv")?;

    let mut files = Vec::with_capacity(module_count);
    for index in 0..module_count {
        let record = &module_bytes[index * MODULE_RECORD_SIZE..(index + 1) * MODULE_RECORD_SIZE];
        let name_ptr = read_pointer(record, 0)?;
        let contents_ptr = read_pointer(record, 8)?;
        let source_map_ptr = read_pointer(record, 16)?;
        let bytecode_ptr = read_pointer(record, 24)?;
        let module_info_ptr = read_pointer(record, 32)?;
        let bytecode_origin_ptr = read_pointer(record, 40)?;
        let encoding = match record[48] {
            0 => BunEncoding::Binary,
            1 => BunEncoding::Latin1,
            2 => BunEncoding::Utf8,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown encoding {value}"
                )))
            }
        };
        let loader = record[49];
        let module_format = match record[50] {
            0 => BunModuleFormat::None,
            1 => BunModuleFormat::Esm,
            2 => BunModuleFormat::Cjs,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown module format {value}"
                )))
            }
        };
        let side = match record[51] {
            0 => BunFileSide::Server,
            1 => BunFileSide::Client,
            value => {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} has unknown file side {value}"
                )))
            }
        };

        let name_bytes = pointer_slice(data, name_ptr, "module name")?;
        if name_ptr.length > 0 {
            let terminator = name_ptr.offset as usize + name_ptr.length as usize;
            if data.get(terminator) != Some(&0) {
                return Err(BunStandaloneError::new(format!(
                    "Bun module {index} name is not NUL-terminated"
                )));
            }
        }
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| BunStandaloneError::new(format!("Bun module {index} name is not UTF-8")))?
            .to_string();
        let contents = pointer_slice(data, contents_ptr, "module contents")?;
        let source_map = pointer_slice(data, source_map_ptr, "module source map")?;
        pointer_slice(data, bytecode_ptr, "module bytecode")?;
        pointer_slice(data, module_info_ptr, "module info")?;
        pointer_slice(data, bytecode_origin_ptr, "bytecode origin path")?;

        let content_start = data_start + contents_ptr.offset as usize;
        let content_end = content_start + contents_ptr.length as usize;
        files.push(BunEmbeddedFile {
            index: index as u32,
            name,
            contents,
            source_map,
            bytecode_size: bytecode_ptr.length,
            module_info_size: module_info_ptr.length,
            encoding,
            loader,
            module_format,
            side,
            is_entry: index == entry_point_id as usize,
            executable_range: content_start..content_end,
        });
    }

    Ok(BunStandalone {
        files,
        entry_point_id,
        compile_exec_argv,
        flags,
        executable_range: data_start..trailer_start + TRAILER.len(),
    })
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
        let second_name = append(&mut data, b"/$bunfs/root/asset.txt", true);
        let second_contents = append(&mut data, b"asset", true);
        let argv = append(&mut data, b"--smol", true);

        let modules_offset = data.len() as u32;
        let mut first = [0u8; MODULE_RECORD_SIZE];
        put_pointer(&mut first, 0, first_name);
        put_pointer(&mut first, 8, first_contents);
        put_pointer(&mut first, 16, first_map);
        first[48] = 1;
        first[49] = 1;
        first[50] = 1;
        first[51] = 0;
        data.extend_from_slice(&first);

        let mut second = [0u8; MODULE_RECORD_SIZE];
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
            length: (MODULE_RECORD_SIZE * 2) as u32,
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
        assert!(standalone.entry_point().is_javascript_like());
        assert!(!standalone.files[1].is_javascript_like());
        assert_eq!(standalone.compile_exec_argv, b"--smol");
        assert_eq!(standalone.flags, 3);
        assert_eq!(
            &executable[standalone.entry_point().executable_range.clone()],
            standalone.entry_point().contents
        );
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
    fn returns_none_without_bun_trailer() {
        assert!(extract_standalone(b"plain JavaScript").unwrap().is_none());
    }
}
