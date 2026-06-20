use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, FileName, SourceMap, Spanned};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};

pub(crate) use crate::utils::swc_safety::apply_fixer;

#[derive(Debug, Clone)]
pub(super) struct ParsedModule {
    pub module: Module,
    pub recoverable_errors: Vec<ParseDiagnostic>,
}

#[derive(Debug, Clone)]
pub(super) struct ParseDiagnostic {
    pub filename: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.filename, self.line, self.column, self.message
        )
    }
}

pub(super) fn parse_js(source: &str, filename: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    Ok(parse_js_with_recovery(source, filename, cm)?.module)
}

pub(super) fn parse_js_with_recovery(
    source: &str,
    filename: &str,
    cm: Lrc<SourceMap>,
) -> Result<ParsedModule> {
    let syntax = detect_syntax(filename);
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
    let mut parser = Parser::new_from(lexer);
    let parsed = match panic::catch_unwind(AssertUnwindSafe(|| parser.parse_module())) {
        Ok(result) => result,
        Err(_) => return Err(anyhow!("SWC parser panicked on {filename}")),
    };
    let parser_errors: Vec<ParseDiagnostic> = parser
        .take_errors()
        .into_iter()
        .map(|error| {
            let loc = cm.lookup_char_pos(error.span().lo());
            ParseDiagnostic {
                filename: filename.to_string(),
                line: loc.line,
                column: loc.col_display + 1,
                message: format!("{:?}", error.kind()),
            }
        })
        .collect();

    match (parsed, parser_errors.is_empty()) {
        (Ok(module), _) => Ok(ParsedModule {
            module,
            recoverable_errors: parser_errors,
        }),
        (Err(error), true) => Err(anyhow!("failed to parse {filename}: {error:?}")),
        (Err(error), false) => Err(anyhow!(
            "failed to parse {filename}: {error:?}; {}",
            parser_errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

pub(super) fn print_js(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
    let mut output = Vec::new();

    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .map_err(|error| anyhow!("failed to print module: {error:?}"))?;
    }

    String::from_utf8(output)
        .map_err(|error| anyhow!("generated output is not valid UTF-8: {error}"))
}

pub(super) fn print_trace_module(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
    let mut printable = module.clone();
    apply_fixer(&mut printable)?;
    print_js(&printable, cm)
}

fn detect_syntax(filename: &str) -> Syntax {
    let path = Path::new(filename);
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("ts") => Syntax::Typescript(TsSyntax {
            tsx: false,
            ..Default::default()
        }),
        Some("tsx") => Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        Some("jsx") => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        _ => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap};

    use super::parse_js;

    #[test]
    fn parse_js_reports_parse_errors_after_parsing() {
        let cm: Lrc<SourceMap> = Default::default();
        let err = parse_js("const = ;", "broken.js", cm).expect_err("invalid JS should fail");

        assert!(
            err.to_string().contains("broken.js"),
            "error should include filename: {err}"
        );
    }
}
