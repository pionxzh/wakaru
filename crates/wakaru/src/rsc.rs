//! Experimental React Server Components payload decoding.
//!
//! A React Server Components payload is the stream a server render sends in
//! React's Flight wire format: one row per line, `<hex id>:<tag><payload>`,
//! where some rows are length-prefixed and may hold newlines. Next.js returns
//! it for a `?_rsc=` request, and embeds the same stream in its page HTML as
//! `self.__next_f.push([1, "…"])` calls. [`decode`] takes either one and
//! splits it into rows.
//!
//! This decodes the transport, not the React tree it describes: model rows keep
//! React's `$` references (`"$L3"`, `"$undefined"`) as plain strings. The
//! output can change in any release.
//!
//! ```
//! let page = r#"<script>(self.__next_f=self.__next_f||[]).push([0])</script>
//! <script>self.__next_f.push([1,":HL[\"/_next/static/css/a.css\",\"style\"]\n1:I[100,[\"200\",\"static/chunks/app/page-a.js\"],\"default\"]\n"])</script>"#;
//! let payload = wakaru::rsc::decode(page.as_bytes()).expect("a payload");
//! assert_eq!(payload.format, wakaru::rsc::Format::NextFlight);
//! assert_eq!(payload.rows[0].id, None);
//! assert_eq!(payload.rows[1].tag, wakaru::rsc::Tag::ModuleReference);
//! assert_eq!(
//!     payload.assets,
//!     ["/_next/static/css/a.css", "static/chunks/app/page-a.js"]
//! );
//! ```

use std::borrow::Cow;

use wakaru_core::rsc as core;

/// The input [`decode`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// The payload itself, as a `?_rsc=` request returns it.
    Raw,
    /// A page or script holding Next.js `self.__next_f.push(...)` calls.
    NextFlight,
}

/// A decoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Payload {
    pub format: Format,
    /// The push calls read, by kind. All zero for a raw payload.
    pub next_entries: NextEntries,
    pub rows: Vec<Row>,
    /// The scripts and stylesheets the payload loads, in payload order without
    /// repeats: the chunks of each client module reference and the file of
    /// each preload or preinit hint, as the payload writes them. Paths in model
    /// rows are page content and are not listed.
    pub assets: Vec<String>,
    /// Push calls that could not be read, then rows that could not be decoded.
    /// The rows around them are kept.
    pub problems: Vec<Problem>,
}

/// How many `self.__next_f.push(...)` calls of each kind the input held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NextEntries {
    /// `[0]`: start of the stream.
    pub bootstrap: usize,
    /// `[1, "…"]`: Flight text.
    pub data: usize,
    /// `[2, …]`: server action form state, not part of the stream.
    pub form_state: usize,
    /// `[3, "…"]`: base64 of binary Flight bytes.
    pub binary: usize,
}

/// One row of the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Row {
    /// The row id, which other rows reference in hex (`"$L1a"`). `None` for a
    /// row written without one: React 19 writes hint rows as `:H…`.
    pub id: Option<u32>,
    pub tag: Tag,
    /// Everything after the tag, or after `<id>:` for a model row. A binary
    /// row holds arbitrary bytes.
    pub payload: Vec<u8>,
    /// The JSON text React parses for this row: the payload after its code
    /// byte for a hint row, otherwise the whole payload. `None` when the row is
    /// not JSON or does not parse.
    pub json: Option<String>,
}

impl Row {
    /// The payload as text, with invalid UTF-8 replaced.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.payload)
    }
}

/// The tag of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Tag {
    /// No tag: a JSON model row.
    Model,
    /// `I`: a client module reference, carrying module id, chunks and export
    /// name.
    ModuleReference,
    /// `H`: a preload hint, such as a stylesheet or script URL.
    Hint,
    /// `E`: an error model.
    Error,
    /// `T`: streamed text, which may hold newlines.
    Text,
    /// Any other tag letter, unchanged.
    Other(char),
}

impl Tag {
    /// The tag letter, or `None` for a model row.
    pub fn letter(&self) -> Option<char> {
        self.core().letter()
    }

    /// `model`, `module-reference`, `hint`, `error`, `text`, or `other`.
    pub fn kind(&self) -> &'static str {
        self.core().kind()
    }

    fn core(&self) -> core::FlightTag {
        match *self {
            Tag::Model => core::FlightTag::Model,
            Tag::ModuleReference => core::FlightTag::Module,
            Tag::Hint => core::FlightTag::Hint,
            Tag::Error => core::FlightTag::Error,
            Tag::Text => core::FlightTag::Text,
            Tag::Other(letter) => core::FlightTag::Other(letter),
        }
    }
}

/// A push call or row that did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Problem {
    pub location: ProblemLocation,
    /// Byte offset of the problem, in the text `location` names.
    pub offset: usize,
    pub message: String,
}

/// The text a [`Problem`] offset counts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProblemLocation {
    /// The input page or script.
    Capture,
    /// The Flight payload: the input itself for [`Format::Raw`], or the
    /// payload rebuilt from the push calls for [`Format::NextFlight`].
    Payload,
}

/// Decode a captured payload, or the payload a captured page or script carries.
///
/// Input that starts with a row header (`1:`, or `:` for an id-less hint row)
/// is a raw payload, even when its rows mention `__next_f`. Other input is
/// searched for `self.__next_f.push(...)` calls; in a page (input that starts
/// with `<`), only inside `<script>` elements. Returns `None` when the input is
/// neither.
pub fn decode(input: &[u8]) -> Option<Payload> {
    let capture = core::decode_capture(input)?;
    let mut next_entries = NextEntries::default();
    for entry in &capture.next_entries {
        match entry.kind {
            core::NextFlightKind::Bootstrap => next_entries.bootstrap += 1,
            core::NextFlightKind::Data => next_entries.data += 1,
            core::NextFlightKind::FormState => next_entries.form_state += 1,
            core::NextFlightKind::Binary => next_entries.binary += 1,
        }
    }
    let assets = capture
        .payload
        .asset_paths()
        .into_iter()
        .map(str::to_string)
        .collect();
    let rows = capture
        .payload
        .rows
        .iter()
        .map(|row| Row {
            id: row.id,
            tag: tag(row.tag),
            json: row.json_text().map(str::to_string),
            payload: row.payload.clone(),
        })
        .collect();
    let problems = capture
        .payload
        .problems
        .into_iter()
        .map(|problem| Problem {
            location: match problem.location {
                core::ProblemLocation::Capture => ProblemLocation::Capture,
                core::ProblemLocation::Payload => ProblemLocation::Payload,
            },
            offset: problem.offset,
            message: problem.message,
        })
        .collect();
    Some(Payload {
        format: match capture.format {
            core::CaptureFormat::Raw => Format::Raw,
            core::CaptureFormat::NextFlight => Format::NextFlight,
        },
        next_entries,
        rows,
        assets,
        problems,
    })
}

fn tag(tag: core::FlightTag) -> Tag {
    match tag {
        core::FlightTag::Model => Tag::Model,
        core::FlightTag::Module => Tag::ModuleReference,
        core::FlightTag::Hint => Tag::Hint,
        core::FlightTag::Error => Tag::Error,
        core::FlightTag::Text => Tag::Text,
        core::FlightTag::Other(letter) => Tag::Other(letter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_without_a_payload_is_none() {
        assert_eq!(decode(b"<!DOCTYPE html><html></html>"), None);
        assert_eq!(decode(b"console.log(1)"), None);
    }

    #[test]
    fn maps_every_part_of_a_raw_payload() {
        let payload =
            decode(b":HL[\"/a.css\",\"style\"]\n1a:T3,a\nb2:[\"$L1a\"]\nzz\n").expect("a payload");

        assert_eq!(payload.format, Format::Raw);
        assert_eq!(payload.next_entries, NextEntries::default());
        assert_eq!(
            payload.rows,
            vec![
                Row {
                    id: None,
                    tag: Tag::Hint,
                    payload: b"L[\"/a.css\",\"style\"]".to_vec(),
                    json: Some("[\"/a.css\",\"style\"]".to_string()),
                },
                Row {
                    id: Some(0x1a),
                    tag: Tag::Text,
                    payload: b"a\nb".to_vec(),
                    json: None,
                },
                Row {
                    id: Some(2),
                    tag: Tag::Model,
                    payload: b"[\"$L1a\"]".to_vec(),
                    json: Some("[\"$L1a\"]".to_string()),
                },
            ]
        );
        assert_eq!(payload.assets, ["/a.css"]);
        assert_eq!(
            payload.problems,
            vec![Problem {
                location: ProblemLocation::Payload,
                offset: 42,
                message: "row id is not hexadecimal: `z`".to_string(),
            }]
        );
        assert_eq!(payload.rows[1].tag.letter(), Some('T'));
        assert_eq!(payload.rows[1].tag.kind(), "text");
    }

    #[test]
    fn counts_the_push_calls_of_a_page() {
        let payload = decode(
            concat!(
                "(self.__next_f=self.__next_f||[]).push([0]);self.__next_f.push([2,null]);",
                r#"self.__next_f.push([1,"1:null\n"]);self.__next_f.push([3,"MjpudWxsCg=="]);"#,
                "self.__next_f.push([7]);",
            )
            .as_bytes(),
        )
        .expect("a payload");

        assert_eq!(payload.format, Format::NextFlight);
        assert_eq!(
            payload.next_entries,
            NextEntries {
                bootstrap: 1,
                data: 1,
                form_state: 1,
                binary: 1,
            }
        );
        assert_eq!(payload.rows.len(), 2);
        assert_eq!(payload.problems.len(), 1);
        assert_eq!(payload.problems[0].location, ProblemLocation::Capture);
    }
}
