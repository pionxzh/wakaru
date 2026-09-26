//! Byte offset → source map position conversion.
//!
//! Source map columns count UTF-16 code units. SWC's `Loc::col_display`
//! counts display width (a CJK character is two), and `Loc::col` counts
//! chars, so neither is a source map column once a line holds non-ASCII
//! text.

/// Line starts and non-ASCII characters of one source, enough to turn a
/// byte offset into a zero-based line and UTF-16 column without rescanning
/// the line (minified inputs are often a single line).
pub(crate) struct LineIndex {
    line_starts: Vec<u32>,
    /// Byte offset of each non-ASCII char, with the running total of
    /// `UTF-8 length - UTF-16 length` over all non-ASCII chars before it.
    narrowing: Vec<(u32, u32)>,
    len: u32,
}

impl LineIndex {
    /// Lines end at `\n`, matching how SWC numbers lines.
    pub(crate) fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        let mut narrowing = Vec::new();
        let mut narrowed_before = 0u32;
        for (offset, ch) in source.char_indices() {
            if ch == '\n' {
                line_starts.push(offset as u32 + 1);
            } else if !ch.is_ascii() {
                narrowing.push((offset as u32, narrowed_before));
                narrowed_before += (ch.len_utf8() - ch.len_utf16()) as u32;
            }
        }
        narrowing.push((u32::MAX, narrowed_before));
        Self {
            line_starts,
            narrowing,
            len: source.len() as u32,
        }
    }

    /// Zero-based line and UTF-16 column of the char starting at `offset`.
    pub(crate) fn position(&self, offset: u32) -> Option<(u32, u32)> {
        if offset > self.len {
            return None;
        }
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let line_start = self.line_starts[line];
        // Both lookups land on a real entry: the sentinel sorts after every
        // offset and carries the total.
        let narrowed_at = |pos: u32| {
            let index = self
                .narrowing
                .partition_point(|&(char_pos, _)| char_pos < pos);
            self.narrowing[index].1
        };
        let narrowed = narrowed_at(offset) - narrowed_at(line_start);
        Some((line as u32, offset - line_start - narrowed))
    }
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    #[test]
    fn position_counts_utf16_units_on_the_offset_line() {
        let index = LineIndex::new("a\n中b𝒳c\nd");
        assert_eq!(index.position(0), Some((0, 0)));
        assert_eq!(index.position(2), Some((1, 0)));
        // "中" is 3 UTF-8 bytes and 1 UTF-16 unit.
        assert_eq!(index.position(5), Some((1, 1)));
        // "𝒳" is 4 UTF-8 bytes and 2 UTF-16 units.
        assert_eq!(index.position(10), Some((1, 4)));
        assert_eq!(index.position(12), Some((2, 0)));
        assert_eq!(index.position(13), Some((2, 1)));
        assert_eq!(index.position(14), None);
    }
}
