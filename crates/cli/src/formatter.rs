use wakaru_formatter::{format_code, format_code_with_source_map, CodeFormatter, FormatResult};

pub fn selected_formatter(enabled: bool) -> CodeFormatter {
    if enabled {
        CodeFormatter::Oxc
    } else {
        CodeFormatter::None
    }
}

pub fn format_cli_output(source: String, filename: &str, formatter: CodeFormatter) -> String {
    report_warning(format_code(source, filename, formatter)).code
}

/// Format decompiled output and carry its source map to the formatted code.
pub fn format_cli_output_with_source_map(
    source: String,
    source_map: Option<String>,
    filename: &str,
    formatter: CodeFormatter,
) -> (String, Option<String>) {
    let result = report_warning(format_code_with_source_map(
        source, source_map, filename, formatter,
    ));
    (result.code, result.source_map)
}

fn report_warning(result: FormatResult) -> FormatResult {
    if let Some(warning) = &result.warning {
        eprintln!(
            "warning: {} formatter failed for {}, preserving output: {}",
            warning.formatter.as_str(),
            warning.filename,
            warning.message
        );
    }
    result
}
