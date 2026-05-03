use std::path::Path;

use anyhow::{anyhow, Result};
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use rayon::prelude::*;

use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_default_rules_with_level, apply_rules_between_with_level,
    apply_rules_range_with_observer_with_level, apply_rules_until, rule_names, ImportDedup,
    RewriteLevel, UnImportRename,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::unpacker::unpack_bundle;

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub filename: String,
    /// Path to a v3 source map file. When provided, enables:
    /// - Import deduplication (merges repeated imports of the same specifier)
    /// - Source-map-driven identifier rename (recovers original variable names)
    pub sourcemap_path: Option<String>,
    /// Run late dead-code-elimination cleanup (`DeadImports`, `DeadDecls`).
    /// Disable this in tests that want to snapshot structural restoration
    /// separately from cleanup.
    pub dead_code_elimination: bool,
    /// Controls how aggressively wakaru recovers likely original source patterns.
    pub level: RewriteLevel,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            filename: String::new(),
            sourcemap_path: None,
            dead_code_elimination: true,
            level: RewriteLevel::Standard,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuleTraceOptions {
    /// First rule to run and trace. When omitted, tracing starts at the
    /// beginning of the normal single-file rule pipeline.
    pub start_from: Option<String>,
    /// Last rule to run and trace. When omitted, tracing stops at the end of
    /// the normal single-file rule pipeline.
    pub stop_after: Option<String>,
    /// When true, only include rules whose rendered output changed.
    pub only_changed: bool,
}

impl Default for RuleTraceOptions {
    fn default() -> Self {
        Self {
            start_from: None,
            stop_after: None,
            only_changed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTraceEvent {
    pub rule: &'static str,
    pub changed: bool,
    pub before: String,
    pub after: String,
}

pub fn decompile(source: &str, options: DecompileOptions) -> Result<String> {
    // Pre-load the source map bytes outside GLOBALS so I/O errors surface early.
    let sourcemap_bytes: Option<Vec<u8>> = options
        .sourcemap_path
        .as_deref()
        .map(std::fs::read)
        .transpose()?;

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, &options.filename, cm.clone())?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        apply_default_rules_with_level(
            &mut module,
            unresolved_mark,
            options.dead_code_elimination,
            options.level,
        );

        // Source-map-enhanced passes (only when --sourcemap is supplied).
        if let Some(bytes) = &sourcemap_bytes {
            let sm = parse_sourcemap(bytes)?;

            // 1. Merge duplicate imports before renaming so that only the canonical
            //    local binding remains when the rename pass runs.
            module.visit_mut_with(&mut ImportDedup);

            // 2. Use source map positions to recover original identifier names.
            apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);

            // 3. Clean up `import { foo as foo }` → `import { foo }` and rename
            //    any remaining aliased imports to their imported name.
            module.visit_mut_with(&mut UnImportRename);
        }

        module.visit_mut_with(&mut fixer(None));

        print_js(&module, cm)
    })
}

pub fn trace_rules(
    source: &str,
    options: DecompileOptions,
    trace_options: RuleTraceOptions,
) -> Result<Vec<RuleTraceEvent>> {
    validate_trace_rule_name("trace start rule", trace_options.start_from.as_deref())?;
    validate_trace_rule_name("trace stop rule", trace_options.stop_after.as_deref())?;

    if unpack_bundle(source).is_some() {
        return Err(anyhow!(
            "rule tracing currently supports single-file inputs only; use normal decompile or unpack for bundles"
        ));
    }

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, &options.filename, cm.clone())?;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

        let mut previous = print_trace_module(&module, cm.clone())?;
        let mut events = Vec::new();
        let mut render_error: Option<anyhow::Error> = None;

        {
            let mut observer = |rule: &'static str, module: &Module| {
                if render_error.is_some() {
                    return;
                }
                match print_trace_module(module, cm.clone()) {
                    Ok(after) => {
                        let changed = after != previous;
                        if changed || !trace_options.only_changed {
                            events.push(RuleTraceEvent {
                                rule,
                                changed,
                                before: previous.clone(),
                                after: after.clone(),
                            });
                        }
                        previous = after;
                    }
                    Err(error) => {
                        render_error = Some(error);
                    }
                }
            };

            apply_rules_range_with_observer_with_level(
                &mut module,
                unresolved_mark,
                trace_options.start_from.as_deref(),
                trace_options.stop_after.as_deref(),
                &mut observer,
                options.dead_code_elimination,
                options.level,
            );
        }

        if let Some(error) = render_error {
            return Err(error);
        }

        Ok(events)
    })
}

fn validate_trace_rule_name(label: &str, rule_name: Option<&str>) -> Result<()> {
    let Some(rule_name) = rule_name else {
        return Ok(());
    };
    if rule_names().contains(&rule_name) {
        Ok(())
    } else {
        Err(anyhow!("unknown {label}: {rule_name}"))
    }
}

/// Render a trace event list as a git-style unified diff log.
///
/// Prints the initial source once, then for each event:
/// - changed: a unified diff against the previous rendering
/// - unchanged: a single header line
///
/// The per-rule "before" string is implied by the previous event's output, so
/// it's never repeated — only the delta is shown.
pub fn format_trace_events(events: &[RuleTraceEvent]) -> String {
    use similar::TextDiff;

    let mut out = String::new();

    let Some(first) = events.first() else {
        return out;
    };

    out.push_str("=== initial ===\n");
    out.push_str(&first.before);
    if !first.before.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    for event in events {
        if !event.changed {
            out.push_str("=== ");
            out.push_str(event.rule);
            out.push_str(" (unchanged) ===\n\n");
            continue;
        }

        out.push_str("=== ");
        out.push_str(event.rule);
        out.push_str(" ===\n");

        let diff = TextDiff::from_lines(&event.before, &event.after);
        let mut unified = diff.unified_diff();
        unified.missing_newline_hint(false);
        for hunk in unified.iter_hunks() {
            out.push_str(&hunk.to_string());
        }
        out.push('\n');
    }

    out
}

pub fn unpack(source: &str, options: DecompileOptions) -> Result<Vec<(String, String)>> {
    match unpack_bundle(source) {
        Some(result) => unpack_multi_module(result.modules, options),
        None => {
            // Not a recognized bundle — treat as a single module
            let code = decompile(source, options)?;
            Ok(vec![("module.js".to_string(), code)])
        }
    }
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns the module code exactly as produced by the bundle detector.
/// Some detectors still do minimal runtime normalization during extraction so
/// their output can be parsed as standalone modules, but cross-module analysis
/// and the normal rule pipeline are skipped.
pub fn unpack_raw(source: &str) -> Result<Vec<(String, String)>> {
    match unpack_bundle(source) {
        Some(result) => Ok(result
            .modules
            .into_iter()
            .map(|module| (module.filename, module.code))
            .collect()),
        None => Ok(vec![("module.js".to_string(), source.to_string())]),
    }
}

/// Multi-module unpack with cross-module late pass.
///
/// Phase 1 (parallel): parse + Stage 1+2 + collect facts (facts only, code discarded)
/// Phase 2 (parallel): full pipeline from scratch with late pass injected at barrier
///
/// Stage 1+2 runs twice per module — once for fact collection, once for the real pipeline.
/// This is necessary because SWC's SyntaxContext must remain continuous across the entire
/// pipeline (re-parsing creates fresh contexts that break rename rules).
fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<Vec<(String, String)>> {
    let sourcemap_bytes: Option<Vec<u8>> = options
        .sourcemap_path
        .as_deref()
        .map(std::fs::read)
        .transpose()?;

    // Phase 1: collect facts (parallel). Run Stage 1+2 on each module and extract
    // import/export facts. The AST is discarded — only facts survive the barrier.
    let phase1: Vec<(String, crate::facts::ModuleFacts)> = modules
        .par_iter()
        .map(|unpacked| {
            let facts = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let Ok(mut module) = parse_js(&unpacked.code, &unpacked.filename, cm) else {
                    return crate::facts::ModuleFacts::default();
                };
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_rules_until(&mut module, unresolved_mark, "UnEsm");
                collect_module_facts(&module)
            });
            (unpacked.filename.clone(), facts)
        })
        .collect();

    let mut module_facts = ModuleFactsMap::new();
    for (filename, facts) in phase1 {
        module_facts.insert(&filename, facts);
    }

    // Phase 2: full pipeline with late pass (parallel). Each module runs the entire
    // pipeline from scratch. Between Stage 2 and Stage 3, the late pass applies
    // cross-module rewrites using the facts collected in Phase 1.
    let facts_ref = &module_facts;
    let pairs: Vec<(String, String)> = modules
        .into_par_iter()
        .map(|unpacked| {
            let code = GLOBALS
                .set(&Default::default(), || {
                    let cm: Lrc<SourceMap> = Default::default();
                    let mut module = parse_js(&unpacked.code, &unpacked.filename, cm.clone())?;
                    let unresolved_mark = Mark::new();
                    let top_level_mark = Mark::new();
                    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

                    // Stage 1+2
                    apply_rules_until(&mut module, unresolved_mark, "UnEsm");

                    // Late pass at the barrier
                    run_reexport_consolidation(&mut module, facts_ref);
                    run_namespace_decomposition(&mut module, facts_ref);

                    // Stage 3+
                    apply_rules_between_with_level(
                        &mut module,
                        unresolved_mark,
                        "UnTemplateLiteral",
                        "UnReturn",
                        options.dead_code_elimination,
                        options.level,
                    );

                    // Source-map-enhanced passes
                    if let Some(bytes) = &sourcemap_bytes {
                        if let Ok(sm) = parse_sourcemap(bytes) {
                            module.visit_mut_with(&mut ImportDedup);
                            apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);
                            module.visit_mut_with(&mut UnImportRename);
                        }
                    }

                    module.visit_mut_with(&mut fixer(None));
                    print_js(&module, cm)
                })
                .unwrap_or(unpacked.code);
            (unpacked.filename, code)
        })
        .collect();

    Ok(pairs)
}

fn parse_js(source: &str, filename: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    let syntax = detect_syntax(filename);
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

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

    String::from_utf8(output)
        .map_err(|error| anyhow!("generated output is not valid UTF-8: {error}"))
}

fn print_trace_module(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
    let mut printable = module.clone();
    printable.visit_mut_with(&mut fixer(None));
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
