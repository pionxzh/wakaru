use std::path::Path;

use anyhow::{anyhow, Result};
use clap::ValueEnum;

const FORMAT_LINE_WIDTH: u16 = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum CliCodeFormatter {
    #[default]
    None,
    Oxc,
}

impl CliCodeFormatter {
    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }
}

pub fn format_cli_output(source: String, filename: &str, formatter: CliCodeFormatter) -> String {
    match formatter {
        CliCodeFormatter::None => source,
        CliCodeFormatter::Oxc => match format_with_oxc(&source, filename) {
            Ok(formatted) => formatted,
            Err(err) => {
                eprintln!(
                    "warning: oxc formatter failed for {filename}, preserving SWC output: {err}"
                );
                source
            }
        },
    }
}

fn format_with_oxc(source: &str, filename: &str) -> Result<String> {
    let allocator = oxc_allocator::Allocator::new();
    let source_type = oxc_source_type(filename);
    let options = oxc_formatter::JsFormatOptions {
        line_width: oxc_formatter_core::LineWidth::try_from(FORMAT_LINE_WIDTH)
            .expect("constant line width should be valid"),
        ..oxc_formatter::JsFormatOptions::default()
    };
    let formatted = oxc_formatter::format(&allocator, source, source_type, options, None)
        .map_err(|err| anyhow!("{err:?}"))?;
    formatted
        .print()
        .map(|printed| printed.into_code())
        .map_err(|err| anyhow!("{err:?}"))
}

fn oxc_source_type(filename: &str) -> oxc_span::SourceType {
    let path = Path::new(filename);
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ts") | Some("mts") | Some("cts") => oxc_span::SourceType::ts(),
        Some("tsx") => oxc_span::SourceType::tsx(),
        _ => oxc_span::SourceType::jsx(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_formatter_returns_source_unchanged() {
        let source = "const  value=1;";
        assert_eq!(
            format_cli_output(source.to_string(), "input.js", CliCodeFormatter::None),
            source
        );
    }

    #[test]
    fn oxc_formatter_failure_preserves_source() {
        let source = "const = ;";
        assert_eq!(
            format_cli_output(source.to_string(), "broken.js", CliCodeFormatter::Oxc),
            source
        );
    }
}
