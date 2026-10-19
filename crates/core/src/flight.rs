//! React Server Components (Flight) payload decoding.
//!
//! A Flight payload is the wire format React streams a server render as: one
//! row per line, `<hex id>:<tag><payload>`. The tag is one letter, and a row
//! with no tag is a JSON model (`2:["$","div",null,{}]`). [`decode`] splits a
//! payload into rows the same way React's client parser does, including the
//! rows that are length-prefixed rather than newline-terminated, so a text row
//! can hold newlines:
//!
//! ```
//! let payload = concat!(
//!     "0:HL[\"/static/alpha.css\",\"style\"]\n",
//!     "1:I[\"static/chunks/app/page.js\",[\"static/chunks/alpha.js\"],\"default\"]\n",
//!     "2:[\"$\",\"div\",null,{\"children\":\"Hello\"}]\n",
//! );
//! let decoded = wakaru_core::flight::decode(payload);
//! assert!(decoded.problems.is_empty());
//! assert_eq!(decoded.rows.len(), 3);
//! assert_eq!(decoded.rows[2].tag, wakaru_core::flight::FlightTag::Model);
//! assert_eq!(
//!     decoded.asset_paths(),
//!     ["/static/alpha.css", "static/chunks/app/page.js", "static/chunks/alpha.js"]
//! );
//! ```
//!
//! Next.js embeds the same stream in its page HTML as
//! `self.__next_f.push([1, "…"])` calls. [`extract_next_flight`] finds those
//! calls, unescapes their JavaScript string literals and rebuilds the payload
//! that [`decode`] takes.
//!
//! Tags follow React's `processFullStringRow`. `I` is a client module
//! reference, `H` a preload hint, `E` an error, `T` streamed text; every other
//! letter is kept as [`FlightTag::Other`] and passed through. Model rows keep
//! React's `$` references (`"$L3"`, `"$undefined"`) as plain strings: this
//! decodes the transport, not the React tree it describes.

use serde_json::Value;

/// Rows React reads as `<hex length>,<data>` instead of up to a newline.
const LENGTH_PREFIXED_TAGS: &[u8] = b"TAOobUSsLlGgMmV";

/// One decoded Flight payload.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlightPayload {
    pub rows: Vec<FlightRow>,
    /// Rows that could not be decoded. The rows before and after them are kept.
    pub problems: Vec<FlightProblem>,
}

/// One Flight row.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightRow {
    /// The hex row id, which other rows reference.
    pub id: u32,
    pub tag: FlightTag,
    /// The row payload as text: everything after the tag, or after `<id>:` for
    /// a model row. Binary length-prefixed rows are decoded lossily.
    pub payload: String,
    /// The payload parsed as JSON: for a hint row the JSON after its code byte,
    /// otherwise the whole payload. `None` when the row is not JSON (a text
    /// row) or does not parse.
    pub json: Option<Value>,
}

impl FlightRow {
    /// Whether React parses this row's payload as JSON and fails on a parse error.
    fn expects_json(&self) -> bool {
        matches!(
            self.tag,
            FlightTag::Model | FlightTag::Module | FlightTag::Hint | FlightTag::Error
        )
    }
}

/// The tag letter of a Flight row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlightTag {
    /// No tag: a JSON model row.
    Model,
    /// `I`: a client module reference, carrying module id, chunks and export
    /// name.
    Module,
    /// `H`: a preload hint, such as a stylesheet or script URL.
    Hint,
    /// `E`: an error model.
    Error,
    /// `T`: streamed text, which may hold newlines.
    Text,
    /// Any other tag, unchanged.
    Other(char),
}

impl FlightTag {
    /// The tag letter, or `None` for a model row.
    pub fn letter(&self) -> Option<char> {
        match self {
            FlightTag::Model => None,
            FlightTag::Module => Some('I'),
            FlightTag::Hint => Some('H'),
            FlightTag::Error => Some('E'),
            FlightTag::Text => Some('T'),
            FlightTag::Other(letter) => Some(*letter),
        }
    }

    /// What React does with a row carrying this tag.
    pub fn kind(&self) -> &'static str {
        match self {
            FlightTag::Model => "model",
            FlightTag::Module => "module-reference",
            FlightTag::Hint => "hint",
            FlightTag::Error => "error",
            FlightTag::Text => "text",
            FlightTag::Other(_) => "other",
        }
    }
}

/// A row that did not decode, and where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlightProblem {
    /// Byte offset the problem was found at: in the payload [`decode`] was
    /// given, or in the captured text [`extract_next_flight`] was given.
    pub offset: usize,
    pub message: String,
}

/// Split a raw Flight payload into rows.
///
/// A malformed row is reported and skipped: the rows around it are still
/// returned. A payload that ends in the middle of a row returns that row only
/// when its framing is complete; otherwise it becomes a problem.
pub fn decode(payload: &str) -> FlightPayload {
    let bytes = payload.as_bytes();
    let mut decoded = FlightPayload::default();
    let mut cursor = 0;

    while cursor < bytes.len() {
        // A newline between rows carries no data. Length-prefixed rows are not
        // newline-terminated themselves, so this only skips stray separators.
        if bytes[cursor] == b'\n' {
            cursor += 1;
            continue;
        }

        match read_row(bytes, cursor) {
            Ok((row, next)) => {
                if row.expects_json() && row.json.is_none() {
                    decoded.problems.push(problem(
                        cursor,
                        format!("row {} payload is not JSON", row.id),
                    ));
                }
                decoded.rows.push(row);
                cursor = next;
            }
            Err(problem) => {
                let restart = bytes[cursor..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| cursor + offset + 1);
                decoded.problems.push(problem);
                cursor = restart;
            }
        }
    }

    decoded
}

fn read_row(bytes: &[u8], start: usize) -> Result<(FlightRow, usize), FlightProblem> {
    let mut cursor = start;
    let mut id: u32 = 0;

    loop {
        let Some(&byte) = bytes.get(cursor) else {
            return Err(problem(start, "row ends after its id, before `:`"));
        };
        if byte == b':' {
            cursor += 1;
            break;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => {
                return Err(problem(
                    start,
                    format!("row id is not hexadecimal: `{}`", escape_byte(byte)),
                ))
            }
        };
        id = id
            .checked_mul(16)
            .and_then(|id| id.checked_add(u32::from(digit)))
            .ok_or_else(|| problem(start, "row id overflows a 32-bit id"))?;
        cursor += 1;
    }

    let Some(&tag_byte) = bytes.get(cursor) else {
        return Err(problem(start, "row ends before its tag"));
    };

    if LENGTH_PREFIXED_TAGS.contains(&tag_byte) {
        cursor += 1;
        let mut length: usize = 0;
        loop {
            let Some(&byte) = bytes.get(cursor) else {
                return Err(problem(start, "length-prefixed row ends before its `,`"));
            };
            cursor += 1;
            if byte == b',' {
                break;
            }
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => {
                    return Err(problem(
                        start,
                        format!("row length is not hexadecimal: `{}`", escape_byte(byte)),
                    ))
                }
            };
            length = length
                .checked_mul(16)
                .and_then(|length| length.checked_add(usize::from(digit)))
                .ok_or_else(|| problem(start, "row length overflows this platform"))?;
        }
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                problem(
                    start,
                    format!(
                        "row declares {length} payload bytes but only {} remain",
                        bytes.len() - cursor
                    ),
                )
            })?;
        let payload = String::from_utf8_lossy(&bytes[cursor..end]).into_owned();
        return Ok((row(id, Some(tag_byte), payload), end));
    }

    // Any other uppercase letter, `#`, `r` or `x` is a tag; anything else is
    // the first byte of a model's JSON.
    let (tag, payload_start) = match tag_byte {
        b'A'..=b'Z' | b'#' | b'r' | b'x' => (Some(tag_byte), cursor + 1),
        _ => (None, cursor),
    };
    let end = bytes[payload_start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| payload_start + offset);
    let payload = String::from_utf8_lossy(&bytes[payload_start..end]).into_owned();
    let next = (end + 1).min(bytes.len());
    Ok((row(id, tag, payload), next))
}

fn row(id: u32, tag: Option<u8>, payload: String) -> FlightRow {
    let tag = match tag {
        None => FlightTag::Model,
        Some(b'I') => FlightTag::Module,
        Some(b'H') => FlightTag::Hint,
        Some(b'E') => FlightTag::Error,
        Some(b'T') => FlightTag::Text,
        Some(other) => FlightTag::Other(other as char),
    };
    let json = match tag {
        FlightTag::Text => None,
        // A hint row is `<code><json>`: React reads the first byte as the hint
        // code and parses the rest.
        FlightTag::Hint => payload
            .get(1..)
            .and_then(|rest| serde_json::from_str(rest).ok()),
        _ => serde_json::from_str(&payload).ok(),
    };
    FlightRow {
        id,
        tag,
        payload,
        json,
    }
}

fn problem(offset: usize, message: impl Into<String>) -> FlightProblem {
    FlightProblem {
        offset,
        message: message.into(),
    }
}

fn escape_byte(byte: u8) -> String {
    if byte.is_ascii_graphic() {
        (byte as char).to_string()
    } else {
        format!("\\x{byte:02x}")
    }
}

/// Asset extensions this decoder treats as a reference to a served file.
const ASSET_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "css", "json", "map", "wasm", "woff", "woff2", "ttf", "otf", "eot", "png",
    "jpg", "jpeg", "gif", "svg", "webp", "avif", "ico", "mp4", "webm",
];

impl FlightPayload {
    /// Every served file path the rows name, in payload order without repeats.
    ///
    /// A client module row names its bundle chunks and a hint row names the
    /// stylesheet or script it preloads, which is what a caller wants to feed
    /// back into bundle unpacking. Anything shaped like a path in a run of JSON
    /// counts: this does not depend on the metadata layout of one React
    /// version.
    pub fn asset_paths(&self) -> Vec<&str> {
        let mut paths = Vec::new();
        for row in &self.rows {
            let Some(json) = &row.json else { continue };
            collect_asset_paths(json, &mut paths);
        }
        paths
    }
}

fn collect_asset_paths<'a>(value: &'a Value, paths: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => {
            if is_asset_path(text) && !paths.contains(&text.as_str()) {
                paths.push(text);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_asset_paths(item, paths);
            }
        }
        Value::Object(fields) => {
            for (_, item) in fields {
                collect_asset_paths(item, paths);
            }
        }
        _ => {}
    }
}

fn is_asset_path(text: &str) -> bool {
    if !text.contains('/') || text.contains(char::is_whitespace) {
        return false;
    }
    let Some((_, extension)) = text.rsplit_once('.') else {
        return false;
    };
    ASSET_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

/// The payload rebuilt from a captured `self.__next_f.push(...)` stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NextFlightCapture {
    pub entries: Vec<NextFlightEntry>,
    /// The Flight stream the data entries carry, in call order, reset by each
    /// bootstrap entry.
    pub payload: String,
    /// Calls that could not be read. The entries around them are kept.
    pub problems: Vec<FlightProblem>,
}

/// One `self.__next_f.push([...])` call on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextFlightEntry {
    pub kind: NextFlightKind,
    /// For a data entry, the Flight text with its JavaScript escapes removed.
    /// For a bootstrap entry, empty. For a form-state entry, the value exactly
    /// as the page spelled it.
    pub text: String,
}

/// The entry kinds Next.js pushes, from `use-flight-response.tsx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextFlightKind {
    /// `0`: start of the stream; discards anything captured before it.
    Bootstrap,
    /// `1`: Flight text.
    Data,
    /// `2`: form state for a server action, not part of the Flight stream.
    FormState,
    /// `3`: base64 of binary Flight bytes.
    Binary,
}

impl NextFlightKind {
    pub fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(NextFlightKind::Bootstrap),
            1 => Some(NextFlightKind::Data),
            2 => Some(NextFlightKind::FormState),
            3 => Some(NextFlightKind::Binary),
            _ => None,
        }
    }
}

/// Find the `self.__next_f.push(...)` calls in page HTML or an extracted
/// script, and rebuild the Flight payload they carry.
///
/// `(self.__next_f=self.__next_f||[]).push(...)` and
/// `self.__next_f.push(...)` are both recognized, as is the same call with any
/// whitespace around it. Text between the calls is ignored, so a whole page or
/// a trimmed copy of one works.
pub fn extract_next_flight(text: &str) -> NextFlightCapture {
    let mut capture = NextFlightCapture::default();
    let mut bytes = Vec::new();
    let mut cursor = 0;

    while let Some((entry, next)) = read_next_push(text, cursor) {
        match entry {
            Ok(entry) => {
                match entry.kind {
                    NextFlightKind::Bootstrap => bytes.clear(),
                    NextFlightKind::Data => bytes.extend_from_slice(entry.text.as_bytes()),
                    NextFlightKind::Binary => match base64_decode(&entry.text) {
                        Ok(chunk) => bytes.extend_from_slice(&chunk),
                        Err(message) => capture.problems.push(problem(cursor, message)),
                    },
                    NextFlightKind::FormState => {}
                }
                capture.entries.push(entry);
            }
            Err(problem) => capture.problems.push(problem),
        }
        cursor = next;
    }

    capture.payload = String::from_utf8_lossy(&bytes).into_owned();
    capture
}

/// The next `__next_f` push at or after `from`, with the offset to resume from.
fn read_next_push(
    text: &str,
    from: usize,
) -> Option<(Result<NextFlightEntry, FlightProblem>, usize)> {
    const NEEDLE: &str = "__next_f";
    let body = text.get(from..)?;
    let marker = body.find(NEEDLE)? + from;
    let push = text.get(marker..)?.find(".push")? + marker;
    let after_push = push + ".push".len();
    let argument = text.get(after_push..)?.trim_start();
    let start = text.len() - argument.len();
    let Some(argument) = argument.strip_prefix('(').map(str::trim_start) else {
        return Some((
            Err(problem(start, "__next_f.push has no argument list")),
            after_push,
        ));
    };
    let start = text.len() - argument.len();

    let Some(end) = scan_balanced(argument) else {
        return Some((
            Err(problem(start, "unterminated __next_f.push argument")),
            text.len(),
        ));
    };
    // Advance past the call so the same push is not read twice when the marker
    // appears on both sides of an assignment, as in
    // `(self.__next_f=self.__next_f||[]).push([1, "…"])`.
    let next = start + end;
    Some((parse_push_argument(&argument[..end], start), next))
}

/// The offset just past the balanced `[...]` that starts `text`.
fn scan_balanced(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for (offset, byte) in text.bytes().enumerate() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' | b'`' => quote = Some(byte),
            b'[' | b'{' | b'(' => depth += 1,
            b']' | b'}' | b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_push_argument(argument: &str, offset: usize) -> Result<NextFlightEntry, FlightProblem> {
    let inner = argument
        .strip_prefix('[')
        .and_then(|argument| argument.strip_suffix(']'))
        .ok_or_else(|| problem(offset, "push argument is not an array"))?;
    let (head, rest) = match inner.split_once(',') {
        Some((head, rest)) => (head, Some(rest.trim_start())),
        None => (inner, None),
    };
    let code: i64 = head
        .trim()
        .parse()
        .map_err(|_| problem(offset, "push entry type is not a number"))?;
    let kind = NextFlightKind::from_code(code)
        .ok_or_else(|| problem(offset, format!("unknown push entry type {code}")))?;

    let text = match (kind, rest) {
        (NextFlightKind::Bootstrap, _) => String::new(),
        (NextFlightKind::FormState, Some(raw)) => raw.to_string(),
        (_, Some(raw)) => {
            let literal = raw
                .strip_prefix('"')
                .and_then(|raw| raw.strip_suffix('"'))
                .ok_or_else(|| problem(offset, "push entry value is not a string"))?;
            unescape_js_string(literal)
                .ok_or_else(|| problem(offset, "push entry value is not a valid string literal"))?
        }
        (_, None) => {
            return Err(problem(
                offset,
                "push entry has a type but no value".to_string(),
            ))
        }
    };

    Ok(NextFlightEntry { kind, text })
}

/// Undo the escapes of a JavaScript string literal body.
fn unescape_js_string(literal: &str) -> Option<String> {
    let mut out = String::with_capacity(literal.len());
    let mut chars = literal.chars();
    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            '0' => out.push('\0'),
            '\n' => {}
            'x' => out.push(char_from_hex(&mut chars, 2)?),
            'u' => out.push(char_from_hex(&mut chars, 4)?),
            other => out.push(other),
        }
    }
    Some(out)
}

fn char_from_hex(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<char> {
    let mut value = 0u32;
    for _ in 0..digits {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    // shortcut: a lone surrogate becomes U+FFFD, as a lossy UTF-8 capture does
    Some(char::from_u32(value).unwrap_or('\u{fffd}'))
}

fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            other => {
                return Err(format!(
                    "binary entry is not base64: `{}`",
                    escape_byte(other)
                ))
            }
        };
        accumulator = ((accumulator << 6) | u32::from(value)) & 0xffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row shapes are real; ids, names and hashes are synthetic.
    const PAYLOAD: &str = concat!(
        "0:HL[\"/static/css/alpha.css\",\"style\"]\n",
        "0:HL[\"static/chunks/alpha.js\",\"script\"]\n",
        "1:I[\"static/chunks/app/page-beta.js\",[\"static/chunks/alpha.js\"],\"default\"]\n",
        "2:[\"$\",\"div\",null,{\"children\":\"Hello\"}]\n",
        "3:E{\"digest\":\"ffff\",\"message\":\"boom\"}\n",
    );

    #[test]
    fn decodes_rows_and_their_tags() {
        let decoded = decode(PAYLOAD);

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 5);
        assert_eq!(decoded.rows[0].tag, FlightTag::Hint);
        assert_eq!(
            decoded.rows[0].payload,
            "L[\"/static/css/alpha.css\",\"style\"]"
        );
        assert_eq!(decoded.rows[2].tag, FlightTag::Module);
        assert_eq!(decoded.rows[2].id, 1);
        assert_eq!(
            decoded.rows[2].json.as_ref().and_then(|json| json.get(2)),
            Some(&Value::String("default".to_string()))
        );
        assert_eq!(
            decoded.rows[0].json.as_ref().and_then(|json| json.get(1)),
            Some(&Value::String("style".to_string()))
        );
        assert_eq!(decoded.rows[3].tag, FlightTag::Model);
        assert_eq!(
            decoded.rows[3].payload,
            "[\"$\",\"div\",null,{\"children\":\"Hello\"}]"
        );
        assert_eq!(decoded.rows[4].tag, FlightTag::Error);
        assert_eq!(
            decoded.rows[4]
                .json
                .as_ref()
                .and_then(|json| json.get("digest")),
            Some(&Value::String("ffff".to_string()))
        );
    }

    #[test]
    fn keeps_newlines_inside_length_prefixed_text_rows() {
        let decoded = decode("4:T5,a\nb\nc\n5:[\"tail\"]\n");

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0].tag, FlightTag::Text);
        assert_eq!(decoded.rows[0].payload, "a\nb\nc");
        assert_eq!(decoded.rows[0].json, None);
        assert_eq!(decoded.rows[1].id, 5);
        assert_eq!(decoded.rows[1].payload, "[\"tail\"]");
    }

    #[test]
    fn keeps_rows_around_a_malformed_row() {
        let decoded = decode("1:[\"a\"]\nzz:[\"b\"]\n2:[\"c\"]\n");

        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0].payload, "[\"a\"]");
        assert_eq!(decoded.rows[1].payload, "[\"c\"]");
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(decoded.problems[0].offset, 8);
        assert_eq!(
            decoded.problems[0].message,
            "row id is not hexadecimal: `z`"
        );
    }

    #[test]
    fn reports_a_row_that_ends_inside_its_payload() {
        let decoded = decode("1:[\"a\"]\n2:Tff,a");

        assert_eq!(decoded.rows.len(), 1);
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(
            decoded.problems[0].message,
            "row declares 255 payload bytes but only 1 remain"
        );
    }

    #[test]
    fn reports_a_model_row_that_is_not_json() {
        let decoded = decode("1:[\"a\",]\n2:[\"b\"]\n");

        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0].json, None);
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(decoded.problems[0].message, "row 1 payload is not JSON");
        assert_eq!(decoded.rows[1].payload, "[\"b\"]");
    }

    #[test]
    fn collects_asset_paths_once_in_payload_order() {
        let decoded = decode(PAYLOAD);

        assert_eq!(
            decoded.asset_paths(),
            [
                "/static/css/alpha.css",
                "static/chunks/alpha.js",
                "static/chunks/app/page-beta.js",
            ]
        );
    }

    /// The shapes Next.js writes into a page: a bootstrap entry, data entries
    /// with escaped Flight rows, and a form-state entry.
    const PAGE: &str = r#"<!DOCTYPE html><html><body>
<script>(self.__next_f=self.__next_f||[]).push([0])</script>
<script>self.__next_f.push([1,"0:HL[\"/static/css/alpha.css\",\"style\"]\n1:I[\"static/chunks/alpha.js\",[],\"default\"]\n"])</script>
<script>self.__next_f.push([2,{"action":"aaaa"}])</script>
<script>self.__next_f.push([1,"2:[\"$\",\"div\",null,{}]\n"])</script>
</body></html>"#;

    #[test]
    fn extracts_the_stream_next_pushes_into_a_page() {
        let capture = extract_next_flight(PAGE);

        assert!(capture.problems.is_empty());
        assert_eq!(
            capture
                .entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [
                NextFlightKind::Bootstrap,
                NextFlightKind::Data,
                NextFlightKind::FormState,
                NextFlightKind::Data,
            ]
        );
        assert_eq!(capture.entries[2].text, r#"{"action":"aaaa"}"#);

        let decoded = decode(&capture.payload);
        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 3);
        assert_eq!(
            decoded.rows[0].payload,
            "L[\"/static/css/alpha.css\",\"style\"]"
        );
        assert_eq!(decoded.rows[2].id, 2);
    }

    #[test]
    fn a_bootstrap_entry_discards_what_came_before_it() {
        let capture = extract_next_flight(concat!(
            r#"self.__next_f.push([1, "1:[\"stale\"]\n"]);"#,
            "self.__next_f.push([0]);",
            r#"self.__next_f.push([1, "2:[\"fresh\"]\n"]);"#,
        ));

        assert_eq!(decode(&capture.payload).rows.len(), 1);
        assert_eq!(decode(&capture.payload).rows[0].payload, "[\"fresh\"]");
    }

    #[test]
    fn reports_a_push_it_cannot_read_and_keeps_the_rest() {
        let capture = extract_next_flight(concat!(
            r#"self.__next_f.push([1, "1:[\"a\"]\n"]);"#,
            r#"self.__next_f.push([9, "x"]);"#,
            r#"self.__next_f.push([1, "2:[\"b\"]\n"]);"#,
        ));

        assert_eq!(capture.entries.len(), 2);
        assert_eq!(capture.problems.len(), 1);
        assert_eq!(capture.problems[0].message, "unknown push entry type 9");
        assert_eq!(decode(&capture.payload).rows.len(), 2);
    }

    #[test]
    fn decodes_binary_entries_before_parsing_them() {
        // "2:[\"b\"]\n" in base64.
        let capture = extract_next_flight("self.__next_f.push([3, \"MjpbImIiXQo=\"]);");

        assert!(capture.problems.is_empty());
        assert_eq!(capture.payload, "2:[\"b\"]\n");
        assert_eq!(decode(&capture.payload).rows[0].payload, "[\"b\"]");
    }
}
