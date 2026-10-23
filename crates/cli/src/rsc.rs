//! `wakaru rsc`: decode a captured React Server Components (Flight) payload.
//!
//! The input is either a raw payload (`1:I[...]` rows, one per line) or a page
//! or script holding Next.js `self.__next_f.push(...)` calls, which are
//! extracted and concatenated before decoding. The output is JSON: the rows,
//! the asset paths they name, and the rows that did not decode.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;
use wakaru_core::rsc::{self, Capture, CaptureFormat, NextFlightKind, ProblemLocation};

use crate::output::write_if_changed;

#[derive(Debug, Clone, Args)]
pub(crate) struct RscArgs {
    /// A raw RSC payload, a page containing `self.__next_f.push(...)` calls,
    /// or a script holding those calls on their own.
    pub(crate) input: PathBuf,

    /// Write the decoded JSON here. Prints to stdout when omitted.
    #[arg(short, long, value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,
}

pub(crate) fn run(args: RscArgs, force: bool) -> Result<()> {
    let input = fs::read(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let Some(capture) = rsc::decode_capture(&input) else {
        bail!(
            "{} holds no React Server Components payload: it does not start with a \
             row such as `1:`, and has no `self.__next_f.push(...)` call",
            args.input.display()
        );
    };
    let report = report(args.input.display().to_string(), &capture);
    let json =
        serde_json::to_string_pretty(&report).context("failed to serialize the report")? + "\n";

    match args.output {
        Some(path) => {
            crate::ensure_output_file(&path, force)?;
            write_if_changed(&path, &json)?;
            println!(
                "{}: {} rows, {} assets, {} problems -> {}",
                report.format,
                report.rows.len(),
                report.assets.len(),
                report.problems.len(),
                path.display()
            );
        }
        None => print!("{json}"),
    }
    Ok(())
}

fn report(source: String, capture: &Capture) -> JsonRscReport {
    let count = |kind| {
        capture
            .next_entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    };
    JsonRscReport {
        source,
        format: match capture.format {
            CaptureFormat::Raw => "raw",
            CaptureFormat::NextFlight => "next.__next_f",
        },
        next_entries: (capture.format == CaptureFormat::NextFlight).then(|| JsonNextEntries {
            bootstrap: count(NextFlightKind::Bootstrap),
            data: count(NextFlightKind::Data),
            form_state: count(NextFlightKind::FormState),
            binary: count(NextFlightKind::Binary),
        }),
        assets: capture
            .payload
            .asset_paths()
            .iter()
            .map(|path| path.to_string())
            .collect(),
        rows: capture.payload.rows.iter().map(JsonRscRow::from).collect(),
        problems: capture
            .payload
            .problems
            .iter()
            .map(JsonRscProblem::from)
            .collect(),
    }
}

#[derive(Serialize)]
struct JsonRscReport {
    source: String,
    /// `next.__next_f` when the input held push calls, else `raw`.
    format: &'static str,
    /// The push entries the input held, by kind. Omitted for a raw payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_entries: Option<JsonNextEntries>,
    /// Scripts and stylesheets the payload loads, in payload order.
    assets: Vec<String>,
    rows: Vec<JsonRscRow>,
    /// Rows and push calls that did not decode. The rest of the output is kept.
    problems: Vec<JsonRscProblem>,
}

#[derive(Serialize)]
struct JsonNextEntries {
    bootstrap: usize,
    data: usize,
    form_state: usize,
    binary: usize,
}

#[derive(Serialize)]
struct JsonRscRow {
    /// The row id in hex, as `$L1a` references spell it; `null` for a row
    /// written without one.
    id: Option<String>,
    /// The tag letter, or `null` for a model row.
    tag: Option<char>,
    /// `model`, `module-reference`, `hint`, `error`, `text`, or `other`.
    kind: &'static str,
    payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json: Option<serde_json::Value>,
}

impl From<&rsc::FlightRow> for JsonRscRow {
    fn from(row: &rsc::FlightRow) -> Self {
        JsonRscRow {
            id: row.id.map(|id| format!("{id:x}")),
            tag: row.tag.letter(),
            kind: row.tag.kind(),
            payload: row.text().into_owned(),
            json: row.json.clone(),
        }
    }
}

#[derive(Serialize)]
struct JsonRscProblem {
    /// `capture` for an offset into the input page or script, `payload` for an
    /// offset into the Flight payload.
    location: &'static str,
    offset: usize,
    message: String,
}

impl From<&rsc::FlightProblem> for JsonRscProblem {
    fn from(problem: &rsc::FlightProblem) -> Self {
        JsonRscProblem {
            location: match problem.location {
                ProblemLocation::Capture => "capture",
                ProblemLocation::Payload => "payload",
            },
            offset: problem.offset,
            message: problem.message.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = concat!(
        r#"<script>(self.__next_f=self.__next_f||[]).push([0])</script>"#,
        r#"<script>self.__next_f.push([1,":HL[\"/static/css/alpha.css\",\"style\"]\n"])</script>"#,
        r#"<script>self.__next_f.push([1,"1a:I[100,[\"200\",\"static/chunks/alpha.js\"],\"default\"]\n"])</script>"#,
    );

    fn report_of(input: &str) -> JsonRscReport {
        let capture = rsc::decode_capture(input.as_bytes()).expect("a payload");
        report("input".to_string(), &capture)
    }

    #[test]
    fn a_raw_payload_stays_a_raw_payload() {
        let report = report_of("1:I[100,[\"200\",\"static/chunks/alpha.js\"],\"default\"]\n");

        assert_eq!(report.format, "raw");
        assert!(report.next_entries.is_none());
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.assets, ["static/chunks/alpha.js"]);
    }

    #[test]
    fn a_page_is_extracted_before_it_is_decoded() {
        let report = report_of(PAGE);

        assert_eq!(report.format, "next.__next_f");
        let next = report.next_entries.expect("push entries");
        assert_eq!((next.bootstrap, next.data), (1, 2));
        assert_eq!(
            report
                .rows
                .iter()
                .map(|row| row.id.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("1a")]
        );
        assert_eq!(
            report.assets,
            ["/static/css/alpha.css", "static/chunks/alpha.js"]
        );
        assert!(report.problems.is_empty());
    }

    #[test]
    fn problems_name_the_text_their_offset_counts_into() {
        let report = report_of(concat!(
            r#"self.__next_f.push([7]);"#,
            r#"self.__next_f.push([1,"1:I(\"a\")\n"]);"#,
        ));

        assert_eq!(
            report
                .problems
                .iter()
                .map(|problem| (problem.location, problem.message.as_str()))
                .collect::<Vec<_>>(),
            [
                ("capture", "unknown push entry type 7"),
                ("payload", "row 1 payload is not JSON"),
            ]
        );
    }
}
