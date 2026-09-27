//! Carry an output source map across the formatter.
//!
//! The formatter rewrites whitespace and a few delimiters (parentheses,
//! trailing commas, `{" "}` in JSX, quotes) but keeps every other token in
//! order. Aligning the token streams before and after formatting gives an
//! exact position table for token starts and ends, which is where the
//! emitter places its mappings. Each mapping moves through that table;
//! mappings at positions the formatter removed are dropped.

use std::borrow::Cow;

use oxc_parser::{config::TokensParserConfig, Kind, Parser};

use std::collections::HashMap;

/// Tokens considered on each side after a mismatch. Formatter edits are
/// local: a delimiter inserted or dropped, or `{" "}`.
const RESYNC_WINDOW: usize = 16;

#[derive(Debug)]
pub(crate) struct LexedToken<'a> {
    key: Cow<'a, str>,
    start: u32,
    end: u32,
}

/// Rewrite `map_json`, whose generated side describes `before`, so that it
/// describes `after`. `None` when either side does not tokenize or the token
/// streams cannot be aligned.
pub(crate) fn remap_source_map(
    before: &str,
    after: &str,
    source_type: oxc_span::SourceType,
    map_json: &str,
) -> Option<String> {
    let allocator = oxc_allocator::Allocator::new();
    let before_tokens = lex(&allocator, before, source_type)?;
    let after_tokens = lex(&allocator, after, source_type)?;
    let pairs = align(&before_tokens, &after_tokens)?;
    let table = PositionTable::new(before, after, &before_tokens, &after_tokens, &pairs);

    let map = sourcemap::SourceMap::from_slice(map_json.as_bytes()).ok()?;
    let mut builder = sourcemap::SourceMapBuilder::new(map.get_file());
    for (id, source) in map.sources().enumerate() {
        let new_id = builder.add_source(source);
        debug_assert_eq!(new_id as usize, id);
        builder.set_source_contents(new_id, map.get_source_contents(id as u32));
    }
    for name in map.names() {
        builder.add_name(name);
    }
    let mut tokens: Vec<_> = map
        .tokens()
        .filter_map(|token| {
            let (line, col) = table.get(token.get_dst_line(), token.get_dst_col())?;
            let raw = token.get_raw_token();
            Some((line, col, raw))
        })
        .collect();
    // The table is monotonic, so this only settles ties.
    tokens.sort_by_key(|&(line, col, _)| (line, col));
    for (line, col, raw) in tokens {
        builder.add_raw(
            line,
            col,
            raw.src_line,
            raw.src_col,
            (raw.src_id != !0).then_some(raw.src_id),
            (raw.name_id != !0).then_some(raw.name_id),
            raw.is_range,
        );
    }

    let mut out = Vec::new();
    builder.into_sourcemap().to_writer(&mut out).ok()?;
    String::from_utf8(out).ok()
}

pub(crate) fn lex<'a>(
    allocator: &'a oxc_allocator::Allocator,
    code: &'a str,
    source_type: oxc_span::SourceType,
) -> Option<Vec<LexedToken<'a>>> {
    let parsed = Parser::new(allocator, code, source_type)
        .with_config(TokensParserConfig)
        .parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return None;
    }
    let mut tokens = Vec::with_capacity(parsed.tokens.len());
    for token in parsed.tokens.iter().filter(|token| !token.kind().is_eof()) {
        let (start, end) = (token.start(), token.end());
        let text = &code[start as usize..end as usize];
        // A template token spans its delimiters (`` `a${ ``, `}b${`, `` }c` ``),
        // but the emitter maps the text between them. Split it so the text's
        // start and end are token boundaries too.
        let close_len = match token.kind() {
            Kind::NoSubstitutionTemplate | Kind::TemplateTail => 1,
            Kind::TemplateHead | Kind::TemplateMiddle => 2,
            kind => {
                tokens.push(LexedToken {
                    key: token_key(kind, text),
                    start,
                    end,
                });
                continue;
            }
        };
        let (content_start, content_end) = (start + 1, end - close_len);
        tokens.push(LexedToken {
            key: Cow::Borrowed(&text[..1]),
            start,
            end: content_start,
        });
        tokens.push(LexedToken {
            key: Cow::Owned(format!(
                "t:{}",
                &code[content_start as usize..content_end as usize]
            )),
            start: content_start,
            end: content_end,
        });
        tokens.push(LexedToken {
            key: Cow::Borrowed(&code[content_end as usize..end as usize]),
            start: content_end,
            end,
        });
    }
    Some(tokens)
}

/// Key under which a token must match its formatted counterpart.
fn token_key(kind: Kind, text: &str) -> Cow<'_, str> {
    match kind {
        // The formatter may switch quotes (re-escaping the other kind) and
        // unquote object keys, so a string matches an identifier with the
        // same text.
        Kind::Str => {
            let inner = &text[1..text.len().saturating_sub(1).max(1)];
            if inner.contains('\\') {
                Cow::Owned(format!(
                    "w:{}",
                    inner.replace("\\\"", "\"").replace("\\'", "'")
                ))
            } else {
                Cow::Owned(format!("w:{inner}"))
            }
        }
        kind if kind.is_identifier_name() => Cow::Owned(format!("w:{text}")),
        kind if kind.is_number() => Cow::Owned(format!("n:{}", text.to_ascii_lowercase())),
        // JSX text is re-wrapped; only its words are stable.
        Kind::JSXText => Cow::Owned(format!(
            "j:{}",
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        )),
        _ => Cow::Borrowed(text),
    }
}

/// Pair up tokens that survive formatting, as `(before_index, after_index)`
/// in increasing order.
///
/// Equal tokens pair greedily. At a mismatch, the longest common subsequence
/// of the next [`RESYNC_WINDOW`] tokens on each side picks the next pair, so
/// neighbouring edits (`a =>` → `(a) =>`) and repeated delimiters resolve
/// the way a full diff would. `None` when a window shares no token, or when
/// under nine in ten tokens pair up: the formatter did more than re-layout.
pub(crate) fn align(
    before: &[LexedToken<'_>],
    after: &[LexedToken<'_>],
) -> Option<Vec<(usize, usize)>> {
    let mut pairs = Vec::with_capacity(before.len().min(after.len()));
    let (mut i, mut j) = (0, 0);
    while i < before.len() && j < after.len() {
        if before[i].key == after[j].key {
            pairs.push((i, j));
            i += 1;
            j += 1;
            continue;
        }
        let (skip_before, skip_after) = first_lcs_pair(
            &before[i..before.len().min(i + RESYNC_WINDOW)],
            &after[j..after.len().min(j + RESYNC_WINDOW)],
        )?;
        i += skip_before;
        j += skip_after;
    }
    if pairs.len() * 10 < before.len().min(after.len()) * 9 {
        return None;
    }
    Some(pairs)
}

/// First pair on a longest common subsequence of `before` and `after`.
fn first_lcs_pair(before: &[LexedToken<'_>], after: &[LexedToken<'_>]) -> Option<(usize, usize)> {
    let width = after.len() + 1;
    // `suffix[x * width + y]`: LCS length of `before[x..]` and `after[y..]`.
    let mut suffix = vec![0u16; (before.len() + 1) * width];
    for x in (0..before.len()).rev() {
        for y in (0..after.len()).rev() {
            suffix[x * width + y] = if before[x].key == after[y].key {
                suffix[(x + 1) * width + y + 1] + 1
            } else {
                suffix[(x + 1) * width + y].max(suffix[x * width + y + 1])
            };
        }
    }
    let (mut x, mut y) = (0, 0);
    while suffix[x * width + y] > 0 {
        if before[x].key == after[y].key
            && suffix[x * width + y] == suffix[(x + 1) * width + y + 1] + 1
        {
            return Some((x, y));
        }
        if suffix[(x + 1) * width + y] >= suffix[x * width + y + 1] {
            x += 1;
        } else {
            y += 1;
        }
    }
    None
}

/// Generated `(line, UTF-16 column)` before formatting → after formatting,
/// for every matched token start and end.
struct PositionTable(HashMap<(u32, u32), (u32, u32)>);

impl PositionTable {
    fn new(
        before: &str,
        after: &str,
        before_tokens: &[LexedToken<'_>],
        after_tokens: &[LexedToken<'_>],
        pairs: &[(usize, usize)],
    ) -> Self {
        // A token's end usually coincides with the next token's start before
        // formatting but not after (`a,b` → `a, b`). A mapping there most
        // likely belongs to the next token, so starts win: sort them after
        // ends at the same offset and keep the last entry.
        let mut offsets: Vec<(u32, bool, u32)> = Vec::with_capacity(pairs.len() * 2);
        for &(b, a) in pairs {
            offsets.push((before_tokens[b].end, false, after_tokens[a].end));
            offsets.push((before_tokens[b].start, true, after_tokens[a].start));
        }
        offsets.sort_unstable_by_key(|&(offset, is_start, _)| (offset, is_start));
        let mut deduped: Vec<(u32, u32)> = Vec::with_capacity(offsets.len());
        for (before_offset, _, after_offset) in offsets {
            match deduped.last_mut() {
                Some(last) if last.0 == before_offset => last.1 = after_offset,
                _ => deduped.push((before_offset, after_offset)),
            }
        }

        let before_positions = line_cols(before, deduped.iter().map(|&(b, _)| b));
        // After-offsets are non-decreasing because the alignment is monotonic.
        let after_positions = line_cols(after, deduped.iter().map(|&(_, a)| a));
        Self(before_positions.into_iter().zip(after_positions).collect())
    }

    fn get(&self, line: u32, col: u32) -> Option<(u32, u32)> {
        self.0.get(&(line, col)).copied()
    }
}

/// Convert non-decreasing byte offsets to 0-based `(line, UTF-16 column)`.
/// Line breaks are `\n`, `\r\n` and `\r`, as the SWC emitter counts them.
fn line_cols(code: &str, offsets: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let (mut pos, mut line, mut col) = (0usize, 0u32, 0u32);
    for offset in offsets {
        let offset = offset as usize;
        debug_assert!(offset >= pos, "offsets must be non-decreasing");
        while pos < offset {
            let ch = code[pos..].chars().next().expect("offset inside code");
            match ch {
                '\r' if bytes.get(pos + 1) == Some(&b'\n') => {
                    pos += 2;
                    line += 1;
                    col = 0;
                    continue;
                }
                '\n' | '\r' => {
                    line += 1;
                    col = 0;
                }
                _ => col += ch.len_utf16() as u32,
            }
            pos += ch.len_utf8();
        }
        out.push((line, col));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys<'a>(allocator: &'a oxc_allocator::Allocator, code: &'a str) -> Vec<LexedToken<'a>> {
        lex(allocator, code, oxc_span::SourceType::jsx()).expect("test code should parse")
    }

    #[test]
    fn line_cols_count_utf16_units_and_crlf() {
        let code = "a\r\n\u{1F600}b\rc";
        let offsets = [0, 3, 7, 9];
        assert_eq!(
            line_cols(code, offsets.into_iter()),
            vec![(0, 0), (1, 0), (1, 2), (2, 0)]
        );
    }

    #[test]
    fn align_skips_inserted_and_dropped_delimiters() {
        let allocator = oxc_allocator::Allocator::new();
        let before = keys(&allocator, "f(a => a, [b,]);");
        let after = keys(&allocator, "f((a) => a, [b]);");
        let pairs = align(&before, &after).expect("streams should align");
        let matched: Vec<_> = pairs
            .iter()
            .map(|&(b, a)| (before[b].key.as_ref(), after[a].key.as_ref()))
            .collect();
        assert!(matched.iter().all(|(b, a)| b == a));
        // Every token except the dropped trailing comma survives.
        assert_eq!(pairs.len(), before.len() - 1);
    }

    #[test]
    fn align_matches_unquoted_keys_and_switched_quotes() {
        let allocator = oxc_allocator::Allocator::new();
        let before = keys(&allocator, r#"x = { "a": "it's", b: "say \"hi\"" };"#);
        let after = keys(&allocator, r#"x = { a: "it's", b: 'say "hi"' };"#);
        let pairs = align(&before, &after).expect("streams should align");
        assert_eq!(pairs.len(), before.len());
    }

    #[test]
    fn lex_splits_template_text_from_its_delimiters() {
        let allocator = oxc_allocator::Allocator::new();
        let code = "x = `a${b}cd${e}f`;";
        let spans: Vec<_> = keys(&allocator, code)
            .iter()
            .map(|token| &code[token.start as usize..token.end as usize])
            .collect();
        assert_eq!(
            spans,
            ["x", "=", "`", "a", "${", "b", "}", "cd", "${", "e", "}", "f", "`", ";"]
        );
    }

    #[test]
    fn align_gives_up_on_unrelated_streams() {
        let allocator = oxc_allocator::Allocator::new();
        let before = keys(&allocator, "a; b; c; d; e; f; g; h; i; j; k; l; m;");
        let after = keys(&allocator, "n; o; p; q; r; s; t; u; v; w; x; y; z;");
        assert!(align(&before, &after).is_none());
    }
}
