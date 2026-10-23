//! React Server Components payload decoding.
//!
//! A React Server Components payload is written in React's Flight wire format:
//! one row per line, `<hex id>:<tag><payload>`. The tag is one letter, and a
//! row with no tag is a JSON model (`2:["$","div",null,{}]`). [`decode`] splits
//! a payload into rows the same way React's client parser does, including the
//! rows that are length-prefixed rather than newline-terminated, so a text row
//! can hold newlines. Lengths count bytes, so the payload is bytes, not text:
//!
//! ```
//! let payload = concat!(
//!     ":HL[\"/_next/static/css/alpha.css\",\"style\"]\n",
//!     "1:I[100,[\"200\",\"static/chunks/app/page-alpha.js\"],\"default\"]\n",
//!     "2:[\"$\",\"div\",null,{\"children\":\"Hello\"}]\n",
//! );
//! let decoded = wakaru_core::rsc::decode(payload.as_bytes());
//! assert!(decoded.problems.is_empty());
//! assert_eq!(decoded.rows.len(), 3);
//! assert_eq!(decoded.rows[0].id, None);
//! assert_eq!(decoded.rows[2].tag, wakaru_core::rsc::FlightTag::Model);
//! assert_eq!(
//!     decoded.asset_paths(),
//!     ["/_next/static/css/alpha.css", "static/chunks/app/page-alpha.js"]
//! );
//! ```
//!
//! Next.js embeds the same stream in its page HTML as
//! `self.__next_f.push([1, "…"])` calls. [`extract_next_flight`] finds those
//! calls, unescapes their JavaScript string literals and rebuilds the payload
//! that [`decode`] takes. [`decode_capture`] tells the two inputs apart.
//!
//! Tags follow React's `processFullStringRow`. `I` is a client module
//! reference, `H` a preload hint, `E` an error, `T` streamed text; every other
//! letter is kept as [`FlightTag::Other`] and passed through. Model rows keep
//! React's `$` references (`"$L3"`, `"$undefined"`) as plain strings: this
//! decodes the transport, not the React tree it describes.

use std::borrow::Cow;
use std::ops::Range;

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
    /// The hex row id, which other rows reference. `None` for a row written
    /// without one: React 19 writes hint rows as `:H…`.
    pub id: Option<u32>,
    pub tag: FlightTag,
    /// The row payload: everything after the tag, or after `<id>:` for a model
    /// row. A length-prefixed binary row holds arbitrary bytes.
    pub payload: Vec<u8>,
    /// The payload parsed as JSON: for a hint row the JSON after its code byte,
    /// otherwise the whole payload. `None` when the row is not JSON (a text
    /// row) or does not parse.
    pub json: Option<Value>,
}

impl FlightRow {
    /// The payload as text, with invalid UTF-8 replaced.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.payload)
    }

    /// The JSON text [`FlightRow::json`] was parsed from.
    pub fn json_text(&self) -> Option<&str> {
        self.json.as_ref()?;
        let start = usize::from(self.tag == FlightTag::Hint);
        std::str::from_utf8(self.payload.get(start..)?).ok()
    }

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

/// A row or push call that did not decode, and where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlightProblem {
    pub location: ProblemLocation,
    /// Byte offset the problem was found at, in the text `location` names.
    pub offset: usize,
    pub message: String,
}

/// The text a [`FlightProblem`] offset counts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemLocation {
    /// The captured page or script [`extract_next_flight`] read.
    Capture,
    /// The Flight payload [`decode`] read: the input itself for a raw payload,
    /// or the payload rebuilt from a page's push calls.
    Payload,
}

/// Split a raw Flight payload into rows.
///
/// A malformed row is reported and skipped: the rows around it are still
/// returned. A payload that ends in the middle of a row returns that row only
/// when its framing is complete; otherwise it becomes a problem.
pub fn decode(bytes: &[u8]) -> FlightPayload {
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
                    let message = match row.id {
                        Some(id) => format!("row {id:x} payload is not JSON"),
                        None => "row payload is not JSON".to_string(),
                    };
                    decoded.problems.push(payload_problem(cursor, message));
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

/// Whether `bytes` starts with a Flight row header, `<hex id>:`. Leading
/// whitespace and a byte order mark are skipped.
pub fn starts_with_row(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let id_len = bytes[start..]
        .iter()
        .take_while(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        .count();
    bytes.get(start + id_len) == Some(&b':')
}

fn read_row(bytes: &[u8], start: usize) -> Result<(FlightRow, usize), FlightProblem> {
    let mut cursor = start;
    let mut id: Option<u32> = None;

    loop {
        let Some(&byte) = bytes.get(cursor) else {
            return Err(payload_problem(start, "row ends after its id, before `:`"));
        };
        if byte == b':' {
            cursor += 1;
            break;
        }
        let Some(digit) = hex_digit(byte) else {
            return Err(payload_problem(
                start,
                format!("row id is not hexadecimal: `{}`", escape_byte(byte)),
            ));
        };
        id = Some(
            id.unwrap_or(0)
                .checked_mul(16)
                .and_then(|id| id.checked_add(u32::from(digit)))
                .ok_or_else(|| payload_problem(start, "row id overflows a 32-bit id"))?,
        );
        cursor += 1;
    }

    let Some(&tag_byte) = bytes.get(cursor) else {
        return Err(payload_problem(start, "row ends before its tag"));
    };

    if LENGTH_PREFIXED_TAGS.contains(&tag_byte) {
        cursor += 1;
        let mut length: usize = 0;
        loop {
            let Some(&byte) = bytes.get(cursor) else {
                return Err(payload_problem(
                    start,
                    "length-prefixed row ends before its `,`",
                ));
            };
            cursor += 1;
            if byte == b',' {
                break;
            }
            let Some(digit) = hex_digit(byte) else {
                return Err(payload_problem(
                    start,
                    format!("row length is not hexadecimal: `{}`", escape_byte(byte)),
                ));
            };
            length = length
                .checked_mul(16)
                .and_then(|length| length.checked_add(usize::from(digit)))
                .ok_or_else(|| payload_problem(start, "row length overflows this platform"))?;
        }
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                payload_problem(
                    start,
                    format!(
                        "row declares {length} payload bytes but only {} remain",
                        bytes.len() - cursor
                    ),
                )
            })?;
        let payload = bytes[cursor..end].to_vec();
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
    let payload = bytes[payload_start..end].to_vec();
    let next = (end + 1).min(bytes.len());
    Ok((row(id, tag, payload), next))
}

fn row(id: Option<u32>, tag: Option<u8>, payload: Vec<u8>) -> FlightRow {
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
            .and_then(|rest| serde_json::from_slice(rest).ok()),
        _ => serde_json::from_slice(&payload).ok(),
    };
    FlightRow {
        id,
        tag,
        payload,
        json,
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn payload_problem(offset: usize, message: impl Into<String>) -> FlightProblem {
    FlightProblem {
        location: ProblemLocation::Payload,
        offset,
        message: message.into(),
    }
}

fn capture_problem(offset: usize, message: impl Into<String>) -> FlightProblem {
    FlightProblem {
        location: ProblemLocation::Capture,
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

/// Extensions of the files [`FlightPayload::asset_paths`] reports: the code
/// and stylesheets a page loads.
const ASSET_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "css"];

/// Hint codes that name a stylesheet or script: `L` preload, `m`
/// modulepreload, `S` preinit stylesheet, `X` preinit script, `M` preinit
/// module script. `D` and `C` name an origin, not a file.
const ASSET_HINT_CODES: &[u8] = b"LmSXM";

impl FlightPayload {
    /// The scripts and stylesheets the payload tells the browser to load, in
    /// payload order without repeats: the chunks of each client module
    /// reference, and the file of each preload or preinit hint. These are what
    /// a caller feeds back into bundle unpacking.
    ///
    /// Model rows are not read: a path there is page content, such as an
    /// image source or a link. A path is reported as written, query included
    /// (`?dpl=…`); only its extension decides whether it counts.
    pub fn asset_paths(&self) -> Vec<&str> {
        let mut paths = Vec::new();
        for row in &self.rows {
            let Some(json) = &row.json else { continue };
            match row.tag {
                FlightTag::Module => {
                    // `[id, chunks, name]` from React 19 and Next.js 14 (with
                    // webpack or Turbopack), or `{"id", "chunks", "name",
                    // "async"}` from Next.js 13.4.
                    let chunks = match json {
                        Value::Array(items) => items.get(1),
                        Value::Object(fields) => fields.get("chunks"),
                        _ => None,
                    };
                    for chunk in chunks.and_then(Value::as_array).into_iter().flatten() {
                        if let Some(chunk) = chunk.as_str() {
                            push_asset(chunk_file(chunk), &mut paths);
                        }
                    }
                }
                FlightTag::Hint => {
                    if !row
                        .payload
                        .first()
                        .is_some_and(|code| ASSET_HINT_CODES.contains(code))
                    {
                        continue;
                    }
                    // `L["/a.css","style"]`, or a bare string for some codes.
                    let href = match json {
                        Value::Array(items) => items.first(),
                        other => Some(other),
                    };
                    if let Some(href) = href.and_then(Value::as_str) {
                        push_asset(href, &mut paths);
                    }
                }
                _ => {}
            }
        }
        paths
    }
}

/// The file a module reference's chunk entry names. Next.js 14 and later list
/// chunk ids and files alternately (`"974", "static/chunks/a.js"`), or only
/// files under Turbopack; Next.js 13.4 joins them (`"974:static/chunks/a.js"`).
fn chunk_file(chunk: &str) -> &str {
    if chunk.contains("://") {
        return chunk;
    }
    chunk.rsplit_once(':').map_or(chunk, |(_, file)| file)
}

fn push_asset<'a>(path: &'a str, paths: &mut Vec<&'a str>) {
    if is_asset_path(path) && !paths.contains(&path) {
        paths.push(path);
    }
}

fn is_asset_path(path: &str) -> bool {
    if path.is_empty() || path.contains(char::is_whitespace) {
        return false;
    }
    let file = path.split(['?', '#']).next().unwrap_or(path);
    let name = file.rsplit('/').next().unwrap_or(file);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && ASSET_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// How [`decode_capture`] read its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    /// The input is the payload itself, as a `?_rsc=` response returns it.
    Raw,
    /// The input is a page or script holding `self.__next_f.push(...)` calls.
    NextFlight,
}

/// A decoded capture: a raw payload, or the payload a page's push calls carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub format: CaptureFormat,
    /// The push calls read, in order. Empty for a raw payload.
    pub next_entries: Vec<NextFlightEntry>,
    /// The `src` of each `<script>` element, in page order without repeats.
    /// Empty unless the input is a page: the bundler runtime and framework
    /// chunks a page loads this way are not named in its payload.
    pub scripts: Vec<String>,
    /// The decoded rows. For a page, `problems` lists the push calls that did
    /// not read before the rows that did not decode.
    pub payload: FlightPayload,
}

/// Decode a captured payload, or the payload a captured page carries.
///
/// Input that starts with a row header (`1:`, or `:` for an id-less hint row)
/// is a raw payload, even when its rows mention `__next_f`. Anything else is
/// searched for `self.__next_f.push(...)` calls. Returns `None` when the input
/// is neither: no row header and no push call.
pub fn decode_capture(input: &[u8]) -> Option<Capture> {
    if starts_with_row(input) {
        return Some(Capture {
            format: CaptureFormat::Raw,
            next_entries: Vec::new(),
            scripts: Vec::new(),
            payload: decode(input),
        });
    }

    let text = String::from_utf8_lossy(input);
    let capture = extract_next_flight(&text);
    if capture.entries.is_empty() && capture.problems.is_empty() {
        return None;
    }
    let mut payload = decode(&capture.payload);
    // The capture's own problems come first: they explain a payload that came
    // out shorter than the page suggested.
    let mut problems = capture.problems;
    problems.append(&mut payload.problems);
    payload.problems = problems;
    Some(Capture {
        format: CaptureFormat::NextFlight,
        next_entries: capture.entries,
        scripts: page_scripts(&text),
        payload,
    })
}

/// The payload rebuilt from a captured `self.__next_f.push(...)` stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NextFlightCapture {
    pub entries: Vec<NextFlightEntry>,
    /// The Flight stream the data and binary entries carry, in call order,
    /// reset by each bootstrap entry.
    pub payload: Vec<u8>,
    /// Calls that could not be read. The entries around them are kept.
    pub problems: Vec<FlightProblem>,
}

/// One `self.__next_f.push([...])` call on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextFlightEntry {
    pub kind: NextFlightKind,
    /// Byte offset of the call's argument in the captured text.
    pub offset: usize,
    /// For a data entry, the Flight text with its JavaScript escapes removed.
    /// For a binary entry, the base64 text. For a bootstrap entry, empty. For
    /// a form-state entry, the value exactly as the page spelled it.
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
/// a trimmed copy of one works. In a page (text that starts with `<`), only
/// `<script>` elements are searched: a call quoted in the page's own text is
/// not one.
pub fn extract_next_flight(text: &str) -> NextFlightCapture {
    let mut capture = NextFlightCapture::default();
    let mut bytes = Vec::new();

    for range in search_ranges(text) {
        let mut cursor = range.start;
        while let Some((entry, next)) = read_next_push(text, cursor, range.end) {
            match entry {
                Ok(entry) => {
                    match entry.kind {
                        NextFlightKind::Bootstrap => bytes.clear(),
                        NextFlightKind::Data => bytes.extend_from_slice(entry.text.as_bytes()),
                        NextFlightKind::Binary => match base64_decode(&entry.text) {
                            Ok(chunk) => bytes.extend_from_slice(&chunk),
                            Err(message) => capture
                                .problems
                                .push(capture_problem(entry.offset, message)),
                        },
                        NextFlightKind::FormState => {}
                    }
                    capture.entries.push(entry);
                }
                Err(problem) => capture.problems.push(problem),
            }
            cursor = next;
        }
    }

    capture.payload = bytes;
    capture
}

/// The parts of `text` that can hold a push call: the contents of each
/// `<script>` element when the text is a page, otherwise all of it.
fn search_ranges(text: &str) -> Vec<Range<usize>> {
    if !is_page(text) {
        return std::iter::once(0..text.len()).collect();
    }
    script_elements(text)
        .into_iter()
        .map(|script| script.content)
        .collect()
}

/// Whether `text` is HTML rather than a script: it starts with `<`.
fn is_page(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
}

/// One `<script>` element of a page, as byte ranges into the page.
struct ScriptElement {
    /// The attributes of the opening tag, after `<script`.
    attributes: Range<usize>,
    content: Range<usize>,
}

fn script_elements(text: &str) -> Vec<ScriptElement> {
    // ASCII lowercasing keeps every byte offset.
    let lower = text.to_ascii_lowercase();
    let mut scripts = Vec::new();
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find("<script") {
        let after_name = cursor + found + "<script".len();
        if !lower[after_name..].starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            cursor = after_name;
            continue;
        }
        let Some(open_end) = opening_tag_end(&text[after_name..]) else {
            break;
        };
        let start = after_name + open_end + 1;
        // A capture cut inside a script still yields the calls before the cut.
        let end = lower[start..]
            .find("</script")
            .map_or(text.len(), |offset| start + offset);
        scripts.push(ScriptElement {
            attributes: after_name..after_name + open_end,
            content: start..end,
        });
        cursor = end;
    }
    scripts
}

/// The offset of the `>` that closes an opening tag, skipping quoted attribute
/// values.
fn opening_tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (offset, byte) in tag.bytes().enumerate() {
        match (quote, byte) {
            (Some(active), _) if byte == active => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'>') => return Some(offset),
            (None, _) => {}
        }
    }
    None
}

/// The `src` of each `<script>` element of a page, in page order without
/// repeats, with HTML character references decoded. Empty for a script.
pub fn page_scripts(text: &str) -> Vec<String> {
    if !is_page(text) {
        return Vec::new();
    }
    let mut sources: Vec<String> = Vec::new();
    for script in script_elements(text) {
        let Some(src) = attribute(&text[script.attributes], "src") else {
            continue;
        };
        let src = decode_character_references(src.trim());
        if !src.is_empty() && !sources.contains(&src) {
            sources.push(src);
        }
    }
    sources
}

/// The value of attribute `wanted` in the attribute text of an opening tag.
fn attribute<'a>(attributes: &'a str, wanted: &str) -> Option<&'a str> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        if rest.is_empty() {
            return None;
        }
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        let mut value = None;
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (found, tail) = match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let end = body.find(quote).unwrap_or(body.len());
                    (&body[..end], body.get(end + 1..).unwrap_or(""))
                }
                _ => {
                    let end = after.find(char::is_whitespace).unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            value = Some(found);
            rest = tail;
        }
        if name.eq_ignore_ascii_case(wanted) {
            return value;
        }
    }
}

/// Decode the character references an attribute value can hold, such as the
/// `&amp;` React writes for `&`.
fn decode_character_references(value: &str) -> String {
    const REFERENCES: &[(&str, char)] = &[
        ("&amp;", '&'),
        ("&quot;", '"'),
        ("&#x27;", '\''),
        ("&#39;", '\''),
        ("&lt;", '<'),
        ("&gt;", '>'),
    ];
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        match REFERENCES
            .iter()
            .find(|(reference, _)| rest.starts_with(reference))
        {
            Some((reference, char)) => {
                out.push(*char);
                rest = &rest[reference.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The next `__next_f` push in `from..limit`, with the offset to resume from.
fn read_next_push(
    text: &str,
    from: usize,
    limit: usize,
) -> Option<(Result<NextFlightEntry, FlightProblem>, usize)> {
    const NEEDLE: &str = "__next_f";
    let mut search = from;
    loop {
        let marker = text.get(search..limit)?.find(NEEDLE)? + search;
        search = marker + NEEDLE.len();
        let Some(open_paren) = push_call_after(text, search) else {
            continue;
        };
        let argument = text[open_paren..limit].trim_start();
        if !argument.starts_with('[') {
            continue;
        }
        let start = limit - argument.len();

        let Some(end) = scan_balanced(argument) else {
            return Some((
                Err(capture_problem(
                    start,
                    "unterminated __next_f.push argument",
                )),
                limit,
            ));
        };
        // Advance past the call so the same push is not read twice when the
        // marker appears on both sides of an assignment, as in
        // `(self.__next_f=self.__next_f||[]).push([1, "…"])`.
        let next = start + end;
        return Some((parse_push_argument(&argument[..end], start), next));
    }
}

/// The offset just past `.push(` when it follows a `__next_f` that ends at
/// `after_marker`, either directly or through `||[])`.
fn push_call_after(text: &str, after_marker: usize) -> Option<usize> {
    let mut rest = text.get(after_marker..)?.trim_start();
    if let Some(fallback) = rest.strip_prefix("||") {
        rest = fallback.trim_start().strip_prefix('[')?;
        rest = rest.trim_start().strip_prefix(']')?;
        rest = rest.trim_start().strip_prefix(')')?.trim_start();
    }
    let rest = rest.strip_prefix(".push")?.trim_start().strip_prefix('(')?;
    Some(text.len() - rest.len())
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
        .ok_or_else(|| capture_problem(offset, "push argument is not an array"))?;
    let (head, rest) = match inner.split_once(',') {
        Some((head, rest)) => (head, Some(rest.trim())),
        None => (inner, None),
    };
    let code: i64 = head
        .trim()
        .parse()
        .map_err(|_| capture_problem(offset, "push entry type is not a number"))?;
    let kind = NextFlightKind::from_code(code)
        .ok_or_else(|| capture_problem(offset, format!("unknown push entry type {code}")))?;

    let text = match (kind, rest) {
        (NextFlightKind::Bootstrap, _) => String::new(),
        (NextFlightKind::FormState, Some(raw)) => raw.to_string(),
        (_, Some(raw)) => {
            let literal = ['"', '\'']
                .into_iter()
                .find_map(|quote| raw.strip_prefix(quote)?.strip_suffix(quote))
                .ok_or_else(|| capture_problem(offset, "push entry value is not a string"))?;
            unescape_js_string(literal).ok_or_else(|| {
                capture_problem(offset, "push entry value is not a valid string literal")
            })?
        }
        (_, None) => {
            return Err(capture_problem(
                offset,
                "push entry has a type but no value".to_string(),
            ))
        }
    };

    Ok(NextFlightEntry { kind, offset, text })
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
            '\r' => {
                let mut ahead = chars.clone();
                if ahead.next() == Some('\n') {
                    chars = ahead;
                }
            }
            '\n' | '\u{2028}' | '\u{2029}' => {}
            'x' => out.push(char::from_u32(hex_value(&mut chars, 2)?)?),
            'u' => {
                let unit = unicode_escape(&mut chars)?;
                // `😀` is one character written as its UTF-16
                // surrogate pair.
                if (0xd800..0xdc00).contains(&unit) {
                    let mut ahead = chars.clone();
                    if ahead.next() == Some('\\') && ahead.next() == Some('u') {
                        if let Some(low) =
                            hex_value(&mut ahead, 4).filter(|low| (0xdc00..0xe000).contains(low))
                        {
                            chars = ahead;
                            out.push(char::from_u32(
                                0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00),
                            )?);
                            continue;
                        }
                    }
                }
                // A lone surrogate becomes U+FFFD, as TextEncoder writes it
                // when Next.js encodes the string for React.
                out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// The value of a `\u` escape after the `u`: four hex digits, or `{…}`.
fn unicode_escape(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let mut ahead = chars.clone();
    if ahead.next() != Some('{') {
        return hex_value(chars, 4);
    }
    let mut value = 0u32;
    let mut digits = 0;
    loop {
        let char = ahead.next()?;
        if char == '}' {
            break;
        }
        value = value.checked_mul(16)? + char.to_digit(16)?;
        digits += 1;
    }
    if digits == 0 || value > 0x10ffff {
        return None;
    }
    *chars = ahead;
    Some(value)
}

fn hex_value(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..digits {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    Some(value)
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

    /// Row shapes from `producer next@15.5.4` (React 19.1); ids, names and
    /// hashes are synthetic.
    const PAYLOAD: &str = concat!(
        ":HL[\"/_next/static/css/alpha.css?dpl=dpl_alpha\",\"style\"]\n",
        "1:I[100,[\"200\",\"static/chunks/app/page-alpha.js?dpl=dpl_alpha\"],\"default\"]\n",
        "2:[\"$\",\"div\",null,{\"children\":\"Hello\"}]\n",
        "3:E{\"digest\":\"ffff\",\"message\":\"boom\"}\n",
    );

    fn payload_of(row: &FlightRow) -> &str {
        std::str::from_utf8(&row.payload).expect("utf-8 payload")
    }

    #[test]
    fn decodes_rows_and_their_tags() {
        let decoded = decode(PAYLOAD.as_bytes());

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 4);
        assert_eq!(decoded.rows[0].tag, FlightTag::Hint);
        assert_eq!(
            payload_of(&decoded.rows[0]),
            "L[\"/_next/static/css/alpha.css?dpl=dpl_alpha\",\"style\"]"
        );
        assert_eq!(
            decoded.rows[0].json_text(),
            Some("[\"/_next/static/css/alpha.css?dpl=dpl_alpha\",\"style\"]")
        );
        assert_eq!(decoded.rows[1].tag, FlightTag::Module);
        assert_eq!(decoded.rows[1].id, Some(1));
        assert_eq!(
            decoded.rows[1].json.as_ref().and_then(|json| json.get(2)),
            Some(&Value::String("default".to_string()))
        );
        assert_eq!(decoded.rows[2].tag, FlightTag::Model);
        assert_eq!(
            payload_of(&decoded.rows[2]),
            "[\"$\",\"div\",null,{\"children\":\"Hello\"}]"
        );
        assert_eq!(decoded.rows[3].tag, FlightTag::Error);
        assert_eq!(
            decoded.rows[3]
                .json
                .as_ref()
                .and_then(|json| json.get("digest")),
            Some(&Value::String("ffff".to_string()))
        );
    }

    #[test]
    fn a_hint_row_keeps_the_id_it_was_written_with() {
        // `producer next@15.5.4` writes hints without an id; `producer
        // next@14.2.33` and `producer next@13.4.19` give each hint its own.
        let decoded =
            decode(b":HL[\"/a.css\",\"style\"]\n0:[\"root\"]\n5:HL[\"/b.css\",\"style\"]\n");

        assert!(decoded.problems.is_empty());
        assert_eq!(
            decoded.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            [None, Some(0), Some(5)]
        );
    }

    #[test]
    fn keeps_newlines_inside_length_prefixed_text_rows() {
        let decoded = decode(b"4:T5,a\nb\nc\n5:[\"tail\"]\n");

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0].tag, FlightTag::Text);
        assert_eq!(payload_of(&decoded.rows[0]), "a\nb\nc");
        assert_eq!(decoded.rows[0].json, None);
        assert_eq!(decoded.rows[1].id, Some(5));
        assert_eq!(payload_of(&decoded.rows[1]), "[\"tail\"]");
    }

    #[test]
    fn a_text_row_length_counts_utf8_bytes() {
        // `producer next@15.5.4`: the length of a `T` row is its UTF-8 length.
        let text = "line 😀 中文";
        let payload = format!("4:T{:x},{text}5:[\"tail\"]\n", text.len());
        let decoded = decode(payload.as_bytes());

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows[0].text(), text);
        assert_eq!(payload_of(&decoded.rows[1]), "[\"tail\"]");
    }

    #[test]
    fn a_binary_row_keeps_its_bytes_and_the_rows_after_it() {
        let decoded = decode(b"1:[\"a\"]\n2:o2,\xff\xfe3:[\"b\"]\n");

        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows.len(), 3);
        assert_eq!(decoded.rows[1].tag, FlightTag::Other('o'));
        assert_eq!(decoded.rows[1].payload, [0xff, 0xfe]);
        assert_eq!(decoded.rows[2].id, Some(3));
        assert_eq!(payload_of(&decoded.rows[2]), "[\"b\"]");
    }

    #[test]
    fn keeps_rows_around_a_malformed_row() {
        let decoded = decode(b"1:[\"a\"]\nzz:[\"b\"]\n2:[\"c\"]\n");

        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(payload_of(&decoded.rows[0]), "[\"a\"]");
        assert_eq!(payload_of(&decoded.rows[1]), "[\"c\"]");
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(decoded.problems[0].offset, 8);
        assert_eq!(decoded.problems[0].location, ProblemLocation::Payload);
        assert_eq!(
            decoded.problems[0].message,
            "row id is not hexadecimal: `z`"
        );
    }

    #[test]
    fn reports_a_row_that_ends_inside_its_payload() {
        let decoded = decode(b"1:[\"a\"]\n2:Tff,a");

        assert_eq!(decoded.rows.len(), 1);
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(
            decoded.problems[0].message,
            "row declares 255 payload bytes but only 1 remain"
        );
    }

    #[test]
    fn reports_a_model_row_that_is_not_json() {
        let decoded = decode(b"1a:[\"a\",]\n2:[\"b\"]\n");

        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0].json, None);
        assert_eq!(decoded.problems.len(), 1);
        assert_eq!(decoded.problems[0].message, "row 1a payload is not JSON");
        assert_eq!(payload_of(&decoded.rows[1]), "[\"b\"]");
    }

    #[test]
    fn assets_are_the_module_chunks_and_hinted_files() {
        let decoded = decode(PAYLOAD.as_bytes());

        assert_eq!(
            decoded.asset_paths(),
            [
                "/_next/static/css/alpha.css?dpl=dpl_alpha",
                "static/chunks/app/page-alpha.js?dpl=dpl_alpha",
            ]
        );
    }

    #[test]
    fn assets_read_the_next_13_module_reference_shape() {
        // `producer next@13.4.19`: an object, and chunks written `<id>:<file>`.
        let decoded = decode(
            concat!(
                "1:HL[\"/_next/static/css/alpha.css\",{\"as\":\"style\"}]\n",
                "2:I{\"id\":100,\"chunks\":[\"200:static/chunks/webpack-alpha.js\",",
                "\"300:static/chunks/app/page-alpha.js\"],\"name\":\"\",\"async\":false}\n",
            )
            .as_bytes(),
        );

        assert!(decoded.problems.is_empty());
        assert_eq!(
            decoded.asset_paths(),
            [
                "/_next/static/css/alpha.css",
                "static/chunks/webpack-alpha.js",
                "static/chunks/app/page-alpha.js",
            ]
        );
    }

    #[test]
    fn assets_read_the_turbopack_module_reference_shape() {
        // `producer next@15.5.4 --turbopack`: chunks are file URLs only, with
        // no chunk ids between them.
        let decoded = decode(
            concat!(
                "2:I[100,[\"/_next/static/chunks/alpha.js?dpl=dpl_alpha\",",
                "\"/_next/static/chunks/beta.js?dpl=dpl_alpha\"],\"default\"]\n",
                ":HL[\"/_next/static/chunks/gamma.css?dpl=dpl_alpha\",\"style\"]\n",
            )
            .as_bytes(),
        );

        assert!(decoded.problems.is_empty());
        assert_eq!(
            decoded.asset_paths(),
            [
                "/_next/static/chunks/alpha.js?dpl=dpl_alpha",
                "/_next/static/chunks/beta.js?dpl=dpl_alpha",
                "/_next/static/chunks/gamma.css?dpl=dpl_alpha",
            ]
        );
    }

    #[test]
    fn assets_leave_out_page_content_and_non_code_hints() {
        let decoded = decode(concat!(
            "1:[\"$\",\"img\",null,{\"src\":\"/images/logo.png\"}]\n",
            "2:[\"$\",\"script\",null,{\"src\":\"/vendor/inline.js\"}]\n",
            ":HL[\"/_next/static/media/font.woff2\",\"font\"]\n",
            ":HC\"https://cdn.example\"\n",
            ":HX[\"/_next/static/chunks/main-alpha.js\"]\n",
            "3:I[100,[\"200\",\"static/chunks/alpha.js\",\"200\",\"static/chunks/alpha.js\"],\"\"]\n",
        )
        .as_bytes());

        assert!(decoded.problems.is_empty());
        assert_eq!(
            decoded.asset_paths(),
            [
                "/_next/static/chunks/main-alpha.js",
                "static/chunks/alpha.js"
            ]
        );
    }

    #[test]
    fn starts_with_row_reads_only_the_row_header() {
        assert!(starts_with_row(b"1:I[100,[],\"\"]\n"));
        assert!(starts_with_row(b":HL[\"/a.css\",\"style\"]\n"));
        assert!(starts_with_row(b"\xef\xbb\xbf\n 1f:null\n"));
        assert!(!starts_with_row(b"<!DOCTYPE html>"));
        assert!(!starts_with_row(b"self.__next_f.push([0])"));
        assert!(!starts_with_row(b"e.x=1"));
    }

    /// The shapes Next.js writes into a page: a bootstrap entry, data entries
    /// with escaped Flight rows, and a form-state entry.
    const PAGE: &str = r#"<!DOCTYPE html><html><body>
<script>(self.__next_f=self.__next_f||[]).push([0])</script>
<script>self.__next_f.push([1,"0:HL[\"/static/css/alpha.css\",\"style\"]\n1:I[100,[],\"default\"]\n"])</script>
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
            payload_of(&decoded.rows[0]),
            "L[\"/static/css/alpha.css\",\"style\"]"
        );
        assert_eq!(decoded.rows[2].id, Some(2));
    }

    #[test]
    fn a_page_is_searched_only_inside_its_scripts() {
        // `producer next@15.5.4` escapes a quoted call in the page text.
        let capture = extract_next_flight(concat!(
            "<!DOCTYPE html><html><body><pre>self.__next_f.push([1, &quot;hi&quot;])</pre>",
            "<script>self.__next_f.push([1,\"1:[\\\"a\\\"]\\n\"])</script>",
            "<script src=\"/a.js\"></script></body></html>",
        ));

        assert!(capture.problems.is_empty());
        assert_eq!(capture.entries.len(), 1);
        assert_eq!(capture.payload, b"1:[\"a\"]\n");
    }

    #[test]
    fn page_scripts_are_the_script_sources_in_page_order() {
        // `producer next@15.5.4`; names and hashes are synthetic.
        let page = concat!(
            "<!DOCTYPE html><html><head>",
            "<link rel=\"preload\" as=\"script\" href=\"/_next/static/chunks/webpack-alpha.js\"/>",
            "<script src=\"/_next/static/chunks/webpack-alpha.js?dpl=dpl_alpha\" id=\"_R_\" async=\"\"></script>",
            "<script src=\"/_next/static/chunks/main-app-alpha.js?dpl=dpl_alpha\" async=\"\"></script>",
            "<script src=\"/_next/static/chunks/polyfills-alpha.js?dpl=dpl_alpha\" noModule=\"\"></script>",
            "</head><body><script>self.__next_f.push([0])</script>",
            "<SCRIPT data-src=\"/lazy.js\" title='a > b' SRC='/a.js?x=1&amp;y=2'></SCRIPT>",
            "<script src=/b.js defer></script>",
            "<script src=\"/_next/static/chunks/main-app-alpha.js?dpl=dpl_alpha\" async=\"\"></script>",
            "</body></html>",
        );

        assert_eq!(
            page_scripts(page),
            [
                "/_next/static/chunks/webpack-alpha.js?dpl=dpl_alpha",
                "/_next/static/chunks/main-app-alpha.js?dpl=dpl_alpha",
                "/_next/static/chunks/polyfills-alpha.js?dpl=dpl_alpha",
                "/a.js?x=1&y=2",
                "/b.js",
            ]
        );
    }

    #[test]
    fn a_script_capture_has_no_page_scripts() {
        assert!(
            page_scripts("self.__next_f.push([0]); var s = '<script src=\"/a.js\">';").is_empty()
        );
    }

    #[test]
    fn a_bootstrap_entry_discards_what_came_before_it() {
        let capture = extract_next_flight(concat!(
            r#"self.__next_f.push([1, "1:[\"stale\"]\n"]);"#,
            "self.__next_f.push([0]);",
            r#"self.__next_f.push([1, "2:[\"fresh\"]\n"]);"#,
        ));

        let decoded = decode(&capture.payload);
        assert_eq!(decoded.rows.len(), 1);
        assert_eq!(payload_of(&decoded.rows[0]), "[\"fresh\"]");
    }

    #[test]
    fn reads_the_entry_shapes_next_writes() {
        // `producer next@14.2.33` pushes `[2,null]` after the bootstrap entry.
        let capture = extract_next_flight(concat!(
            "(self.__next_f = self.__next_f || []).push([0]);self.__next_f.push([2,null])\n",
            r#"self.__next_f.push( [1, "1:[\"a\"]\n" ] );"#,
            r#"self.__next_f.push([1, '2:["b"]\n']);"#,
            "self.__next_f = self.__next_f || [];",
        ));

        assert!(capture.problems.is_empty());
        assert_eq!(
            capture
                .entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [
                NextFlightKind::Bootstrap,
                NextFlightKind::FormState,
                NextFlightKind::Data,
                NextFlightKind::Data,
            ]
        );
        assert_eq!(capture.payload, b"1:[\"a\"]\n2:[\"b\"]\n");
    }

    #[test]
    fn joins_an_escaped_surrogate_pair_before_counting_bytes() {
        // A four-byte character, written the way an ASCII-only serializer
        // escapes it.
        let capture = extract_next_flight(concat!(
            r#"self.__next_f.push([1,"4:T4,😀"]);"#,
            r#"self.__next_f.push([1,"5:[\"tail\",\"\u{1F600}\"]\n"]);"#,
        ));

        assert!(capture.problems.is_empty());
        let decoded = decode(&capture.payload);
        assert!(decoded.problems.is_empty());
        assert_eq!(decoded.rows[0].text(), "😀");
        assert_eq!(payload_of(&decoded.rows[1]), "[\"tail\",\"😀\"]");
    }

    #[test]
    fn a_lone_surrogate_becomes_a_replacement_character() {
        let capture = extract_next_flight(r#"self.__next_f.push([1,"1:[\"\ud83d!\"]\n"]);"#);

        assert_eq!(capture.payload, "1:[\"\u{fffd}!\"]\n".as_bytes());
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
        assert_eq!(capture.problems[0].location, ProblemLocation::Capture);
        assert_eq!(decode(&capture.payload).rows.len(), 2);
    }

    #[test]
    fn decodes_binary_entries_before_parsing_them() {
        // "2:[\"b\"]\n" in base64.
        let capture = extract_next_flight("self.__next_f.push([3, \"MjpbImIiXQo=\"]);");

        assert!(capture.problems.is_empty());
        assert_eq!(capture.payload, b"2:[\"b\"]\n");
        assert_eq!(payload_of(&decode(&capture.payload).rows[0]), "[\"b\"]");
    }

    #[test]
    fn a_binary_entry_problem_points_at_its_push() {
        let text = "self.__next_f.push([1, \"1:null\\n\"]);\nself.__next_f.push([3, \"!\"]);";
        let capture = extract_next_flight(text);

        assert_eq!(capture.problems.len(), 1);
        assert_eq!(capture.problems[0].offset, text.rfind('[').unwrap());
    }

    #[test]
    fn a_raw_payload_that_quotes_a_push_call_stays_raw() {
        // `producer next@15.5.4`: a `?_rsc=` response whose page text quotes a
        // push call.
        let capture = decode_capture(
            b"1:[\"$\",\"pre\",null,{\"children\":\"self.__next_f.push([1, \\\"hi\\\"])\"}]\n",
        )
        .expect("a payload");

        assert_eq!(capture.format, CaptureFormat::Raw);
        assert!(capture.next_entries.is_empty());
        assert!(capture.scripts.is_empty());
        assert_eq!(capture.payload.rows.len(), 1);
        assert!(capture.payload.problems.is_empty());
    }

    #[test]
    fn a_page_capture_reports_push_problems_before_row_problems() {
        let capture = decode_capture(
            concat!(
                r#"self.__next_f.push([7]);"#,
                r#"self.__next_f.push([1,"1:I(\"a\")\n"]);"#,
            )
            .as_bytes(),
        )
        .expect("a page");

        assert_eq!(capture.format, CaptureFormat::NextFlight);
        assert_eq!(
            capture
                .payload
                .problems
                .iter()
                .map(|problem| (problem.location, problem.message.as_str()))
                .collect::<Vec<_>>(),
            [
                (ProblemLocation::Capture, "unknown push entry type 7"),
                (ProblemLocation::Payload, "row 1 payload is not JSON"),
            ]
        );
    }

    #[test]
    fn a_page_without_push_calls_is_not_a_capture() {
        let page = b"<!DOCTYPE html>\n<html><body><div id=\"__next\"></div>\n<script id=\"__NEXT_DATA__\" type=\"application/json\">{\"props\":{}}</script>\n</body></html>\n";

        assert_eq!(decode_capture(page), None);
        assert_eq!(
            decode_capture(b"self.__next_f = self.__next_f || [];"),
            None
        );
    }

    #[test]
    fn a_capture_cut_inside_a_push_reports_the_cut() {
        let capture = decode_capture(
            b"<html><script>self.__next_f.push([1,\"1:null\\n\"])</script><script>self.__next_f.push([1,\"2:nu",
        )
        .expect("a page");

        assert_eq!(capture.payload.rows.len(), 1);
        assert_eq!(capture.payload.problems.len(), 1);
        assert_eq!(
            capture.payload.problems[0].message,
            "unterminated __next_f.push argument"
        );
        assert!(capture.scripts.is_empty());
    }
}
