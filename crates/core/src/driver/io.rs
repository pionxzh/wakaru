use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, BytePos, FileName, LineCol, SourceMap, Spanned};
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
    parse_js_with_recovery_owned(source.to_string(), filename, cm)
}

pub(super) fn parse_js_with_recovery_owned(
    source: String,
    filename: &str,
    cm: Lrc<SourceMap>,
) -> Result<ParsedModule> {
    let syntax = detect_syntax(filename);
    let fm = cm.new_source_file(FileName::Custom(filename.to_string()).into(), source);

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

pub(super) fn print_js_with_srcmap(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Result<(String, Vec<(BytePos, LineCol)>)> {
    let mut output = Vec::new();
    let mut srcmap_buf: Vec<(BytePos, LineCol)> = Vec::new();

    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, Some(&mut srcmap_buf)),
        };
        emitter
            .emit_module(module)
            .map_err(|error| anyhow!("failed to print module: {error:?}"))?;
    }

    let code = String::from_utf8(output)
        .map_err(|error| anyhow!("generated output is not valid UTF-8: {error}"))?;
    Ok((code, srcmap_buf))
}

/// Build a v3 source map JSON string from the raw emitter mappings.
///
/// `mappings` are `(input_byte_pos, output_line_col)` entries collected by
/// `JsWriter`. `cm` is the SWC `SourceMap` used during parsing (holds the
/// original source). `output_filename` is the name used for the `"file"` field.
pub(super) fn build_output_sourcemap(
    mappings: &[(BytePos, LineCol)],
    cm: &SourceMap,
    output_filename: &str,
) -> Result<String> {
    let mut builder = sourcemap::SourceMapBuilder::new(Some(output_filename));

    for &(byte_pos, ref out_loc) in mappings {
        // DUMMY_SP positions (BytePos(0)) have no meaningful source location.
        if byte_pos.0 == 0 {
            continue;
        }
        let loc = cm.lookup_char_pos(byte_pos);
        let source_name = match &*loc.file.name {
            FileName::Custom(name) => name.as_str(),
            _ => continue,
        };
        let src_id = builder.add_source(source_name);
        if !loc.file.src.is_empty() {
            builder.set_source_contents(src_id, Some(loc.file.src.as_ref()));
        }
        builder.add_raw(
            out_loc.line,
            out_loc.col,
            (loc.line - 1) as u32,
            loc.col_display as u32,
            Some(src_id),
            None,
            false,
        );
    }

    let srcmap = builder.into_sourcemap();
    let mut buf = Vec::new();
    srcmap
        .to_writer(&mut buf)
        .map_err(|e| anyhow!("failed to serialize source map: {e}"))?;
    String::from_utf8(buf).map_err(|e| anyhow!("source map is not valid UTF-8: {e}"))
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
