use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};
use wakaru_rs::{
    apply_rules_between, apply_rules_until_with_level, decompile, trace_rules, DecompileOptions,
    RuleTraceEvent, RuleTraceOptions,
    RewriteLevel,
};

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

/// Run the decompile pipeline up through `stop_after_rule`, then emit.
/// Rule names match struct names (e.g. "SmartInline", "UnEsm").
/// Second passes use suffixed names: "UnWebpackInterop2", "UnIife2".
#[allow(dead_code)]
pub fn render_pipeline_until(source: &str, stop_after_rule: &str) -> String {
    render_pipeline_until_with_level(source, stop_after_rule, RewriteLevel::Standard)
}

#[allow(dead_code)]
pub fn render_pipeline_until_with_level(
    source: &str,
    stop_after_rule: &str,
    level: RewriteLevel,
) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module_with_filename(source, "fixture.js", cm.clone());

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        apply_rules_until_with_level(&mut module, unresolved_mark, stop_after_rule, true, level);
        module.visit_mut_with(&mut fixer(None));

        emit_module(&module, cm)
    })
}

/// Run only the rules from `start_from` through `stop_after` (inclusive).
/// Useful for testing a rule's behavior given realistic pre-processed input
/// without the full pipeline's downstream effects.
#[allow(dead_code)]
pub fn render_pipeline_between(source: &str, start_from: &str, stop_after: &str) -> String {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_module_with_filename(source, "fixture.js", cm.clone());

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        apply_rules_between(&mut module, unresolved_mark, start_from, stop_after);
        module.visit_mut_with(&mut fixer(None));

        emit_module(&module, cm)
    })
}

#[allow(dead_code)]
pub fn trace_pipeline(source: &str, options: RuleTraceOptions) -> Vec<RuleTraceEvent> {
    trace_rules(
        source,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
        options,
    )
    .expect("trace should succeed")
}

#[allow(dead_code)]
pub fn changed_rules(source: &str) -> Vec<&'static str> {
    trace_pipeline(
        source,
        RuleTraceOptions {
            only_changed: true,
            ..Default::default()
        },
    )
    .into_iter()
    .map(|event| event.rule)
    .collect()
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
