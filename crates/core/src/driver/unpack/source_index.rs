//! Compose unpack output source maps back to the original input.
//!
//! Phase 2 re-parses each module's extracted code, so its emitter mappings
//! point into that intermediate text. The extra hop runs through
//! [`InputOffsets`] (intermediate offset → input offset, recorded by the
//! extractor) and then [`LineIndex`] (input offset → line and UTF-16 column).

use anyhow::{anyhow, Result};
use swc_core::common::{BytePos, LineCol, SourceMap};

use super::super::io::add_innermost_mappings;
use super::super::line_index::LineIndex;
use crate::unpacker::InputOffsets;

/// The input a module was extracted from, kept only when output source maps
/// are requested: its name for `sources` and its line index.
pub(crate) struct InputOrigin {
    pub(crate) name: String,
    lines: LineIndex,
}

impl InputOrigin {
    pub(crate) fn new(name: String, source: &str) -> Self {
        // SWC drops a leading byte order mark before assigning positions, and
        // so does a browser decoding the script, so offsets start after it.
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        Self {
            name,
            lines: LineIndex::new(source),
        }
    }
}

/// Build a v3 map from the Phase 2 emitter mappings in which every mapped
/// position points into `origin`. Positions without a proven input offset
/// (synthesized code, printer-only tokens) stay unmapped. The map names the
/// input and omits `sourcesContent`: embedding the whole input in every
/// module's map would repeat it once per module.
pub(crate) fn build_composed_output_sourcemap(
    mappings: &[(BytePos, LineCol)],
    cm: &SourceMap,
    output_filename: &str,
    offsets: InputOffsets<'_>,
    origin: &InputOrigin,
) -> Result<String> {
    let mut builder = sourcemap::SourceMapBuilder::new(Some(output_filename));
    let src_id = builder.add_source(&origin.name);
    let resolved = mappings.iter().filter_map(|&(byte_pos, out_loc)| {
        if byte_pos.0 == 0 {
            return None;
        }
        let extracted_offset = cm.lookup_byte_offset(byte_pos).pos.0;
        let (line, col) = offsets
            .input_offset(extracted_offset)
            .and_then(|input_offset| origin.lines.position(input_offset))?;
        Some((out_loc, line, col, src_id))
    });
    add_innermost_mappings(&mut builder, resolved);
    let mut buf = Vec::new();
    builder
        .into_sourcemap()
        .to_writer(&mut buf)
        .map_err(|e| anyhow!("failed to serialize source map: {e}"))?;
    String::from_utf8(buf).map_err(|e| anyhow!("source map is not valid UTF-8: {e}"))
}
