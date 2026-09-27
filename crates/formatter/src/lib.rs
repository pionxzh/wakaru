use std::path::Path;

mod source_map;

const FORMAT_LINE_WIDTH: u16 = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CodeFormatter {
    #[default]
    None,
    Oxc,
}

impl CodeFormatter {
    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Oxc => "oxc",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatWarning {
    pub formatter: CodeFormatter,
    pub filename: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatResult {
    pub code: String,
    /// The output source map, rewritten to describe `code`. `None` when no
    /// map was passed in.
    pub source_map: Option<String>,
    pub warning: Option<FormatWarning>,
}

pub fn format_code(source: String, filename: &str, formatter: CodeFormatter) -> FormatResult {
    format_code_with_source_map(source, None, filename, formatter)
}

/// Format `source` and rewrite `source_map`, whose generated side is
/// `source`, to describe the formatted code.
///
/// When the map cannot be carried across formatting, the source stays
/// unformatted with its original map, and the result carries a warning: a
/// map that points at the wrong output positions is worse than no formatting.
pub fn format_code_with_source_map(
    source: String,
    source_map: Option<String>,
    filename: &str,
    formatter: CodeFormatter,
) -> FormatResult {
    let unformatted = |source_map, message: String| FormatResult {
        code: source.clone(),
        source_map,
        warning: Some(FormatWarning {
            formatter,
            filename: filename.to_string(),
            message,
        }),
    };
    match formatter {
        CodeFormatter::None => FormatResult {
            code: source,
            source_map,
            warning: None,
        },
        CodeFormatter::Oxc => match format_with_oxc(&source, filename) {
            Err(message) => unformatted(source_map, message),
            Ok(code) => match source_map {
                None => FormatResult {
                    code,
                    source_map: None,
                    warning: None,
                },
                Some(map) => match source_map::remap_source_map(
                    &source,
                    &code,
                    oxc_source_type(filename),
                    &map,
                ) {
                    Some(remapped) => FormatResult {
                        code,
                        source_map: Some(remapped),
                        warning: None,
                    },
                    None => unformatted(
                        Some(map),
                        "could not carry the source map across formatting".to_string(),
                    ),
                },
            },
        },
    }
}

fn format_with_oxc(source: &str, filename: &str) -> Result<String, String> {
    let allocator = oxc_allocator::Allocator::new();
    let source_type = oxc_source_type(filename);
    let options = oxc_formatter::JsFormatOptions {
        line_width: oxc_formatter_core::LineWidth::try_from(FORMAT_LINE_WIDTH)
            .expect("constant line width should be valid"),
        ..oxc_formatter::JsFormatOptions::default()
    };
    let formatted = oxc_formatter::format(&allocator, source, source_type, options)
        .map_err(|err| format!("{err:?}"))?;
    formatted
        .print()
        .map(|printed| printed.into_code())
        .map_err(|err| format!("{err:?}"))
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
            format_code(source.to_string(), "input.js", CodeFormatter::None),
            FormatResult {
                code: source.to_string(),
                source_map: None,
                warning: None,
            }
        );
    }

    #[test]
    fn oxc_formatter_formats_source() {
        let result = format_code(
            "const  value=1;".to_string(),
            "input.js",
            CodeFormatter::Oxc,
        );
        assert_eq!(result.warning, None);
        assert_eq!(result.code, "const value = 1;\n");
    }

    #[test]
    fn oxc_formatter_failure_preserves_source() {
        let source = "const = ;";
        let result = format_code(source.to_string(), "broken.js", CodeFormatter::Oxc);
        assert_eq!(result.code, source);
        assert!(result.warning.is_some());
    }
}
