//! `wakaru rsc`: decode a captured React Server Components (Flight) payload.
//!
//! The input is either a raw payload (`1:I[...]` rows, one per line) or a page
//! or script holding Next.js `self.__next_f.push(...)` calls, which are
//! extracted and concatenated before decoding. The output is JSON: the rows,
//! the asset paths they name, and the rows that did not decode.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use wakaru_core::rsc::{self as flight, FlightPayload};

use crate::output::write_if_changed;

#[derive(Debug, Clone, Args)]
pub(crate) struct RscArgs {
    /// A raw Flight payload, a page containing `self.__next_f.push(...)`
    /// calls, or a script holding those calls on their own.
    pub(crate) input: PathBuf,

    /// Write the decoded JSON here. Prints to stdout when omitted.
    #[arg(short, long, value_name = "FILE")]
    pub(crate) output: Option<PathBuf>,
}

pub(crate) fn run(args: RscArgs) -> Result<()> {
    let text = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let decoded = decode_input(&text);

    let report = JsonFlightReport {
        source: args.input.display().to_string(),
        format: decoded.format,
        next_flight: decoded.next_flight,
        assets: decoded
            .payload
            .asset_paths()
            .iter()
            .map(|p| p.to_string())
            .collect(),
        rows: decoded
            .payload
            .rows
            .iter()
            .map(JsonFlightRow::from)
            .collect(),
        problems: decoded
            .payload
            .problems
            .iter()
            .map(JsonFlightProblem::from)
            .collect(),
    };
    let json =
        serde_json::to_string_pretty(&report).context("failed to serialize the report")? + "\n";

    match args.output {
        Some(path) => {
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

struct DecodedInput {
    format: &'static str,
    next_flight: Option<JsonNextFlight>,
    payload: FlightPayload,
}

fn decode_input(text: &str) -> DecodedInput {
    let capture = flight::extract_next_flight(text);
    // No readable push call means the input is the payload itself, not a page
    // or a script that carries one.
    if capture.entries.is_empty() && capture.problems.is_empty() {
        return DecodedInput {
            format: "raw-flight",
            next_flight: None,
            payload: flight::decode(text),
        };
    }

    let mut payload = flight::decode(&capture.payload);
    // Report the capture's own problems first: they explain a payload that came
    // out shorter than the page suggested.
    let mut problems = capture.problems;
    problems.append(&mut payload.problems);
    payload.problems = problems;

    let count = |kind| {
        capture
            .entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    };
    DecodedInput {
        format: "next.__next_f",
        next_flight: Some(JsonNextFlight {
            bootstrap: count(flight::NextFlightKind::Bootstrap),
            data: count(flight::NextFlightKind::Data),
            form_state: count(flight::NextFlightKind::FormState),
            binary: count(flight::NextFlightKind::Binary),
        }),
        payload,
    }
}

#[derive(Serialize)]
struct JsonFlightReport {
    source: String,
    /// `next.__next_f` when the input held push calls, else `raw-flight`.
    format: &'static str,
    /// The push entries the input held, by kind. Omitted for a raw payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_flight: Option<JsonNextFlight>,
    /// Served files the payload references, in payload order.
    assets: Vec<String>,
    rows: Vec<JsonFlightRow>,
    /// Rows and push calls that did not decode. The rest of the output is kept.
    problems: Vec<JsonFlightProblem>,
}

#[derive(Serialize)]
struct JsonNextFlight {
    bootstrap: usize,
    data: usize,
    form_state: usize,
    binary: usize,
}

#[derive(Serialize)]
struct JsonFlightRow {
    id: u32,
    /// The tag letter, or `null` for a model row.
    tag: Option<char>,
    /// `model`, `module-reference`, `hint`, `error`, `text`, or `other`.
    kind: &'static str,
    payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json: Option<serde_json::Value>,
}

impl From<&flight::FlightRow> for JsonFlightRow {
    fn from(row: &flight::FlightRow) -> Self {
        JsonFlightRow {
            id: row.id,
            tag: row.tag.letter(),
            kind: row.tag.kind(),
            payload: row.payload.clone(),
            json: row.json.clone(),
        }
    }
}

#[derive(Serialize)]
struct JsonFlightProblem {
    offset: usize,
    message: String,
}

impl From<&flight::FlightProblem> for JsonFlightProblem {
    fn from(problem: &flight::FlightProblem) -> Self {
        JsonFlightProblem {
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
        r#"<script>self.__next_f.push([1,"0:HL[\"static/css/alpha.css\",\"style\"]\n"])</script>"#,
        r#"<script>self.__next_f.push([1,"1:I[\"static/chunks/alpha.js\",[],\"default\"]\n"])</script>"#,
    );

    #[test]
    fn a_raw_payload_stays_a_raw_payload() {
        let decoded = decode_input("1:I[\"static/chunks/alpha.js\",[],\"default\"]\n");

        assert_eq!(decoded.format, "raw-flight");
        assert!(decoded.next_flight.is_none());
        assert_eq!(decoded.payload.rows.len(), 1);
        assert_eq!(decoded.payload.asset_paths(), ["static/chunks/alpha.js"]);
    }

    #[test]
    fn a_page_is_extracted_before_it_is_decoded() {
        let decoded = decode_input(PAGE);

        assert_eq!(decoded.format, "next.__next_f");
        let next = decoded.next_flight.expect("push entries");
        assert_eq!((next.bootstrap, next.data), (1, 2));
        assert_eq!(decoded.payload.rows.len(), 2);
        assert_eq!(
            decoded.payload.asset_paths(),
            ["static/css/alpha.css", "static/chunks/alpha.js"]
        );
        assert!(decoded.payload.problems.is_empty());
    }

    #[test]
    fn capture_problems_come_before_payload_problems() {
        let decoded = decode_input(concat!(
            r#"self.__next_f.push([7]);"#,
            r#"self.__next_f.push([1,"1:I(\"a\")\n"]);"#,
        ));

        let messages: Vec<&str> = decoded
            .payload
            .problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect();
        assert_eq!(
            messages,
            ["unknown push entry type 7", "row 1 payload is not JSON",]
        );
    }
}
