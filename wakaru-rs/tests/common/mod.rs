use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use wakaru_rs::{decompile, DecompileOptions};

#[allow(dead_code)]
pub fn render(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
        },
    )
    .expect("decompile should succeed")
}

#[allow(dead_code)]
pub fn normalize(input: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_module(input, cm.clone());
        emit_module(&module, cm)
    })
}

#[allow(dead_code)]
pub fn assert_eq_normalized(actual: &str, expected: &str) {
    assert_eq!(normalize(actual), normalize(expected));
}

#[allow(dead_code)]
pub fn assert_normalized_eq(output: &str, expected: &str) {
    assert_eq_normalized(output, expected);
}

#[allow(dead_code)]
pub fn assert_compact_eq(output: &str, expected: &str) {
    assert_eq_normalized(output, expected);
}

fn parse_module(code: &str, cm: Lrc<SourceMap>) -> Module {
    let fm = cm.new_source_file(FileName::Custom("normalize.js".to_string()).into(), code.to_string());
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

    let parser_errors = parser.take_errors();
    assert!(
        parser_errors.is_empty(),
        "failed to parse for normalization: {:?}",
        parser_errors
    );

    parser
        .parse_module()
        .expect("failed to parse module for normalization")
}

fn emit_module(module: &Module, cm: Lrc<SourceMap>) -> String {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut output, None),
        };
        emitter
            .emit_module(module)
            .expect("failed to emit module for normalization");
    }
    String::from_utf8(output).expect("normalization output is not utf-8")
}
