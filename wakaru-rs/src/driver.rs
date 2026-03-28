use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use crate::rules::apply_default_rules;
use crate::unpacker::unpack_webpack4;

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub filename: String,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            filename: "input.js".to_string(),
        }
    }
}

pub fn decompile(source: &str, options: DecompileOptions) -> Result<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, &options.filename, cm.clone())?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        apply_default_rules(&mut module, unresolved_mark);

        module.visit_mut_with(&mut fixer(None));

        print_js(&module, cm)
    })
}

pub fn unpack(source: &str, options: DecompileOptions) -> Result<Vec<(String, String)>> {
    match unpack_webpack4(source) {
        Some(result) => {
            let mut pairs = Vec::new();
            for module in result.modules {
                let code = decompile(
                    &module.code,
                    DecompileOptions {
                        filename: module.filename.clone(),
                    },
                )
                .unwrap_or(module.code);
                pairs.push((module.filename, code));
            }
            Ok(pairs)
        }
        None => {
            // Not a recognized bundle — treat as a single module
            let code = decompile(source, options)?;
            Ok(vec![("module.js".to_string(), code)])
        }
    }
}

fn parse_js(source: &str, filename: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    let syntax = detect_syntax(filename);
    let fm = cm.new_source_file(FileName::Custom(filename.to_string()).into(), source.to_string());

    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(&*fm), None);
    let mut parser = Parser::new_from(lexer);
    let parser_errors: Vec<String> = parser
        .take_errors()
        .into_iter()
        .map(|error| format!("{error:?}"))
        .collect();
    if !parser_errors.is_empty() {
        return Err(anyhow!(
            "failed to parse {filename}: {}",
            parser_errors.join("; ")
        ));
    }

    parser
        .parse_module()
        .map_err(|error| anyhow!("failed to parse {filename}: {error:?}"))
}

fn print_js(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
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

    String::from_utf8(output).map_err(|error| anyhow!("generated output is not valid UTF-8: {error}"))
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
        _ => Syntax::Es(EsSyntax::default()),
    }
}
