use clap::ValueEnum;
use wakaru_formatter::{format_code, CodeFormatter};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum CliCodeFormatter {
    #[default]
    None,
    Oxc,
}

impl CliCodeFormatter {
    pub fn is_enabled(self) -> bool {
        CodeFormatter::from(self).is_enabled()
    }
}

impl From<CliCodeFormatter> for CodeFormatter {
    fn from(value: CliCodeFormatter) -> Self {
        match value {
            CliCodeFormatter::None => Self::None,
            CliCodeFormatter::Oxc => Self::Oxc,
        }
    }
}

pub fn format_cli_output(source: String, filename: &str, formatter: CliCodeFormatter) -> String {
    let result = format_code(source, filename, formatter.into());
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
