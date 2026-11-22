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

    /// Byte offset of the char at zero-based `line` and UTF-16 column `col`,
    /// the inverse of [`Self::position`]. `source` must be the text this
    /// index was built from. `None` when the line does not exist, the column
    /// is past the end of the line, or it falls inside a surrogate pair.
    pub(crate) fn offset(&self, source: &str, line: u32, col: u32) -> Option<u32> {
        let line = line as usize;
        let line_start = *self.line_starts.get(line)?;
        let line_end = self
            .line_starts
            .get(line + 1)
            .map_or(self.len, |&next| next - 1);
        // Non-ASCII chars from the line start on; the sentinel ends the slice.
        let first = self
            .narrowing
            .partition_point(|&(char_pos, _)| char_pos < line_start);
        let rest = &self.narrowing[first..];
        let base = rest[0].1;
        // UTF-16 column of each of these chars rises with its index, so the
        // first one at or past `col` bounds the chars that come before it.
        let column_of = |(char_pos, narrowed): (u32, u32)| {
            u64::from(char_pos - line_start) - u64::from(narrowed - base)
        };
        let index = rest.partition_point(|&entry| column_of(entry) < u64::from(col));
        let offset = u64::from(line_start) + u64::from(col) + u64::from(rest.get(index)?.1 - base);
        if offset > u64::from(line_end) || !source.is_char_boundary(offset as usize) {
            return None;
        }
        Some(offset as u32)
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

    #[test]
    fn offset_inverts_position_on_every_char() {
        let source = "a\n中b𝒳c\né\nd";
        let index = LineIndex::new(source);
        for (offset, _) in source.char_indices().chain([(source.len(), ' ')]) {
            let (line, col) = index.position(offset as u32).unwrap();
            assert_eq!(
                index.offset(source, line, col),
                Some(offset as u32),
                "{line}:{col}"
            );
        }
    }

    #[test]
    fn offset_rejects_columns_off_the_line_or_inside_a_surrogate_pair() {
        let source = "a\n中b𝒳c\nd";
        let index = LineIndex::new(source);
        // "𝒳" covers UTF-16 columns 2 and 3 of line 1.
        assert_eq!(index.offset(source, 1, 3), None);
        // Line 1 ends at column 5; column 6 would be on the next line.
        assert_eq!(index.offset(source, 1, 5), Some(11));
        assert_eq!(index.offset(source, 1, 6), None);
        assert_eq!(index.offset(source, 0, 2), None);
        assert_eq!(index.offset(source, 3, 0), None);
        assert_eq!(index.offset(source, 1, u32::MAX), None);
    }
}
