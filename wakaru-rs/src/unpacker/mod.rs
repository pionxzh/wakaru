pub mod browserify;
pub mod esbuild;
pub mod webpack4;
pub mod webpack5;

use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

pub struct UnpackedModule {
    pub id: String,
    pub is_entry: bool,
    pub code: String,
    pub filename: String,
}

pub struct UnpackResult {
    pub modules: Vec<UnpackedModule>,
}

pub fn unpack_bundle(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_es_module(source, "bundle.js", cm.clone()).ok()?;

        webpack5::detect_from_module(&module, cm.clone())
            .or_else(|| webpack4::detect_from_module(&module, cm.clone()))
            .or_else(|| webpack5::detect_chunk_from_module(&module, cm.clone()))
            .or_else(|| browserify::detect_from_module(&module, cm.clone()))
            .or_else(|| esbuild::detect_from_module(&module, cm))
    })
}

pub(crate) fn parse_es_module(
    source: &str,
    filename: &str,
    cm: Lrc<SourceMap>,
) -> anyhow::Result<Module> {
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| anyhow::anyhow!("parse error: {e:?}"))
}

pub fn unpack_webpack4(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract(source)
}

pub fn unpack_webpack4_raw(source: &str) -> Option<UnpackResult> {
    webpack4::detect_and_extract_raw(source)
}
