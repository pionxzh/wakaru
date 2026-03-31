use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use wakaru_rs::{decompile, DecompileOptions};

#[allow(dead_code)]
pub fn render_pipeline(source: &str) -> String {
    decompile(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed")
}

#[allow(dead_code)]
pub fn render(source: &str) -> String {
    render_pipeline(source)
}

#[allow(dead_code)]
pub fn render_rule<R, F>(source: &str, build_rule: F) -> String
where
    R: VisitMut,
    F: FnOnce(Mark) -> R,
{
    render_rule_with_filename(source, "fixture.js", build_rule)
}

#[allow(dead_code)]
pub fn render_rule_with_filename<R, F>(source: &str, filename: &str, build_rule: F) -> String
where
    R: VisitMut,
    F: FnOnce(Mark) -> R,
{
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module_with_filename(source, filename, cm.clone());

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let mut rule = build_rule(unresolved_mark);
        module.visit_mut_with(&mut rule);
        module.visit_mut_with(&mut fixer(None));

        emit_module(&module, cm)
    })
}

#[allow(dead_code)]
pub fn normalize(input: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = parse_module_with_filename(input, "normalize.js", cm.clone());
        emit_module(&module, cm)
    })
}

#[allow(dead_code)]
pub fn assert_eq_normalized(actual: &str, expected: &str) {
    assert_eq!(normalize(actual), normalize(expected));
}

fn parse_module_with_filename(code: &str, filename: &str, cm: Lrc<SourceMap>) -> Module {
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        code.to_string(),
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
