use wakaru_formatter::{format_code, CodeFormatter};

pub fn selected_formatter(enabled: bool) -> CodeFormatter {
    if enabled {
        CodeFormatter::Oxc
    } else {
        CodeFormatter::None
    }
}

pub fn format_cli_output(source: String, filename: &str, formatter: CodeFormatter) -> String {
    let result = format_code(source, filename, formatter);
    if let Some(warning) = result.warning {
        eprintln!(
            "warning: {} formatter failed for {}, preserving output: {}",
            warning.formatter.as_str(),
            warning.filename,
            warning.message
        );
    }
    result.code
}
