use swc_core::ecma::ast::{JSXAttrValue, Str};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

/// Decodes `\uXXXX`, `\u{...}` and `\xXX` escapes of printable non-ASCII
/// characters in string literals (`"\u4e2d\u6587"` → `"中文"`).
///
/// Only the literal's raw spelling changes; the value, quote style and every
/// other escape stay as written. Characters that cannot be printed or are
/// invisible in source (lone surrogates, controls, separators, whitespace,
/// format and bidi controls, variation selectors, private use) stay escaped.
/// Template literals are never touched: their raw text is observable through
/// `String.raw` and tag functions.
pub struct UnStringEscape;

impl VisitMut for UnStringEscape {
    fn visit_mut_str(&mut self, str: &mut Str) {
        let Some(raw) = &str.raw else {
            return;
        };
        if let Some(decoded) = decode_printable_escapes(raw) {
            str.raw = Some(decoded.into());
        }
    }

    fn visit_mut_jsx_attr_value(&mut self, value: &mut JSXAttrValue) {
        // JSX attribute strings do not process escapes, so their raw text is
        // the value itself.
        if matches!(value, JSXAttrValue::Str(_)) {
            return;
        }
        value.visit_mut_children_with(self);
    }
}

/// Returns the rewritten raw spelling, or `None` when nothing was decoded.
fn decode_printable_escapes(raw: &str) -> Option<String> {
    if !raw.contains("\\u") && !raw.contains("\\x") {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut changed = false;
    let mut i = 0;
    while i < raw.len() {
        if bytes[i] != b'\\' {
            let ch = raw[i..].chars().next().expect("in bounds");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if let Some((code_point, len)) = parse_escape(&raw[i..]) {
            // Two `\uXXXX` halves next to a `-` are usually the endpoints of
            // a regex range built as a string (`"\uD800-\uDB7F\uDC00-\uDFFF"`),
            // not one character; joining them hides the range.
            let is_range_pair =
                len == 12 && (raw[..i].ends_with('-') || raw[i + len..].starts_with('-'));
            if let Some(ch) = char::from_u32(code_point)
                .filter(|&ch| is_printable(ch))
                .filter(|_| !is_range_pair)
            {
                out.push(ch);
                changed = true;
                i += len;
                continue;
            }
            out.push_str(&raw[i..i + len]);
            i += len;
            continue;
        }
        // Any other escape: copy the backslash and the escaped character.
        out.push('\\');
        i += 1;
        if let Some(ch) = raw[i..].chars().next() {
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    changed.then_some(out)
}

/// Parses a `\xXX`, `\uXXXX` (joining a surrogate pair written as two
/// escapes) or `\u{...}` escape at the start of `s`, returning the code point
/// and the escape's byte length.
fn parse_escape(s: &str) -> Option<(u32, usize)> {
    let rest = s.strip_prefix('\\')?;
    if let Some(hex) = rest.strip_prefix('x') {
        return Some((parse_hex(hex.get(..2)?)?, 4));
    }
    let rest = rest.strip_prefix('u')?;
    if let Some(braced) = rest.strip_prefix('{') {
        let end = braced.find('}')?;
        let code_point = parse_hex(&braced[..end])?;
        return (code_point <= 0x10FFFF).then_some((code_point, 3 + end + 1));
    }
    let unit = parse_hex(rest.get(..4)?)?;
    if (0xD800..=0xDBFF).contains(&unit) {
        if let Some(low) = rest
            .get(4..6)
            .filter(|prefix| *prefix == "\\u")
            .and_then(|_| rest.get(6..10))
            .and_then(parse_hex)
            .filter(|low| (0xDC00..=0xDFFF).contains(low))
        {
            let code_point = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            return Some((code_point, 12));
        }
    }
    Some((unit, 6))
}

fn parse_hex(hex: &str) -> Option<u32> {
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

fn is_printable(ch: char) -> bool {
    !ch.is_ascii()
        && !ch.is_control()
        && !ch.is_whitespace()
        && !matches!(
            ch as u32,
            0x00AD
                | 0x034F
                | 0x061C
                | 0x115F..=0x1160
                | 0x17B4..=0x17B5
                | 0x180B..=0x180F
                | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x206F
                | 0x3164
                | 0xFE00..=0xFE0F
                | 0xFEFF
                | 0xFFA0
                | 0xFFF0..=0xFFFF
                | 0xE000..=0xF8FF
                | 0x1BCA0..=0x1BCA3
                | 0x1D173..=0x1D17A
                // Unassigned planes, tags and supplementary private use.
                | 0x323B0..=0x10FFFF
        )
}

#[cfg(test)]
mod tests {
    use super::decode_printable_escapes;

    #[test]
    fn decodes_only_printable_non_ascii_escapes() {
        assert_eq!(
            decode_printable_escapes(r#""\u4e2d""#).as_deref(),
            Some(r#""中""#)
        );
        assert_eq!(decode_printable_escapes(r#""\\u4e2d""#), None);
        assert_eq!(decode_printable_escapes(r#""\u0041\x41""#), None);
        assert_eq!(decode_printable_escapes(r#""\uD800""#), None);
        assert_eq!(decode_printable_escapes(r#""\u{110000}""#), None);
        assert_eq!(decode_printable_escapes(r#""\u{4e2d""#), None);
        assert_eq!(
            decode_printable_escapes(r#""\uD800-\uDB7F\uDC00-\uDFFF""#),
            None
        );
    }
}
