use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, Spanned, GLOBALS};
use swc_core::ecma::ast::{
    BindingIdent, ClassDecl, ForInStmt, ForOfStmt, ForStmt, ImportDecl, ImportSpecifier, Module,
    ObjectPatProp, Pat, VarDecl, VarDeclKind,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_default_rules_with_level, apply_rules_between_with_level,
    apply_rules_range_with_observer_with_level, apply_rules_until, rule_names, ImportDedup,
    RewriteLevel, UnEsm, UnImportRename,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::unpacker::{scope_hoist, try_unpack_bundle, UnpackResult};

#[derive(Debug, Clone)]
pub struct DecompileOptions {
    pub filename: String,
    /// Raw bytes of a v3 source map. When provided, enables:
    /// - Import deduplication (merges repeated imports of the same specifier)
    /// - Source-map-driven identifier rename (recovers original variable names)
    pub sourcemap: Option<Vec<u8>>,
    /// Run late dead-code-elimination cleanup (`DeadImports`, `DeadDecls`).
    /// Disable this in tests that want to snapshot structural restoration
    /// separately from cleanup.
    pub dead_code_elimination: bool,
    /// Controls how aggressively wakaru recovers likely original source patterns.
    pub level: RewriteLevel,
    /// When true and no bundle format is detected, attempt heuristic splitting
    /// of scope-hoisted bundles (Rollup/Vite/flat esbuild).
    pub heuristic_split: bool,
    /// Run post-transform diagnostic checks (lexical use-before-declaration,
    /// output parse verification). Results are returned as warnings.
    pub diagnostics: bool,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            filename: String::new(),
            sourcemap: None,
            dead_code_elimination: false,
            level: RewriteLevel::Standard,
            heuristic_split: false,
            diagnostics: false,
        }
    }
}

/// Result of an unpack operation: the extracted modules plus any non-fatal
/// warnings (e.g. per-module parse failures that fell back to raw code).
#[derive(Debug, Clone, Default)]
pub struct UnpackOutput {
    pub modules: Vec<(String, String)>,
    pub warnings: Vec<UnpackWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackWarning {
    pub filename: String,
    pub kind: UnpackWarningKind,
    pub message: String,
}

impl UnpackWarning {
    fn new(
        filename: impl Into<String>,
        kind: UnpackWarningKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            filename: filename.into(),
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for UnpackWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.filename, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnpackWarningKind {
    RawNormalizationFailed,
    FactCollectionParseFailed,
    DecompileFailed,
    InputParseRecovered,
    TdzViolation,
    DuplicateDeclaration,
    OutputParseRecovered,
    OutputParseFailed,
}

impl UnpackWarningKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawNormalizationFailed => "raw_normalization_failed",
            Self::FactCollectionParseFailed => "fact_collection_parse_failed",
            Self::DecompileFailed => "decompile_failed",
            Self::InputParseRecovered => "input_parse_recovered",
            Self::TdzViolation => "tdz_violation",
            Self::DuplicateDeclaration => "duplicate_declaration",
            Self::OutputParseRecovered => "output_parse_recovered",
            Self::OutputParseFailed => "output_parse_failed",
        }
    }

    /// Diagnostic warnings signal potential issues in transform output
    /// but do not indicate data loss or parse failure during unpack.
    pub fn is_diagnostic(self) -> bool {
        matches!(self, Self::InputParseRecovered | Self::TdzViolation)
    }

    pub fn is_error(self) -> bool {
        !self.is_diagnostic()
    }
}

impl UnpackOutput {
    /// True when there are non-diagnostic warnings (parse failures, decompile
    /// errors). Diagnostic warnings like TDZ violations are excluded.
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.kind.is_error())
    }
}

/// Result of a single-file decompile: the output code plus any non-fatal
/// warnings (e.g. TDZ violations detected after transformation).
#[derive(Debug, Clone, Default)]
pub struct DecompileOutput {
    pub code: String,
    pub warnings: Vec<UnpackWarning>,
}

impl DecompileOutput {
    pub fn has_errors(&self) -> bool {
        self.warnings.iter().any(|w| w.kind.is_error())
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

pub fn decompile(source: &str, options: DecompileOptions) -> Result<DecompileOutput> {
    let span = tracing::info_span!("decompile", filename = %options.filename);
    let _enter = span.enter();

    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let parsed = {
            let span = tracing::info_span!("parse");
            let _enter = span.enter();
            parse_js_with_recovery(source, &options.filename, cm.clone())?
        };
        let mut module = parsed.module;

        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        {
            let span = tracing::info_span!("resolver");
            let _enter = span.enter();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        }

        {
            let span = tracing::info_span!("rules");
            let _enter = span.enter();
            apply_default_rules_with_level(
                &mut module,
                unresolved_mark,
                options.dead_code_elimination,
                options.level,
            );
        }

        // Source-map-enhanced passes (only when sourcemap bytes are supplied).
        if let Some(bytes) = &options.sourcemap {
            let span = tracing::info_span!("sourcemap_renames");
            let _enter = span.enter();
            let sm = parse_sourcemap(bytes)?;
            module.visit_mut_with(&mut ImportDedup);
            apply_sourcemap_renames(&mut module, &sm, &cm, unresolved_mark);
            module.visit_mut_with(&mut UnImportRename);
        }

        let mut warnings = if options.diagnostics {
            let mut warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
            warnings.extend(collect_tdz_warnings(&module, &options.filename));
            warnings.extend(collect_duplicate_declaration_warnings(
                &module,
                &options.filename,
            ));
            warnings
        } else {
            Vec::new()
        };

        {
            let span = tracing::info_span!("fixer");
            let _enter = span.enter();
            module.visit_mut_with(&mut fixer(None));
        }

        let code = {
            let span = tracing::info_span!("emit");
            let _enter = span.enter();
            print_js(&module, cm)?
        };

        if options.diagnostics {
            warnings.extend(verify_output_parses(&code, &options.filename));
        }

        Ok(DecompileOutput { code, warnings })
    })
}

pub fn trace_rules(
    source: &str,
    options: DecompileOptions,
    trace_options: RuleTraceOptions,
) -> Result<Vec<RuleTraceEvent>> {
    validate_trace_rule_name("trace start rule", trace_options.start_from.as_deref())?;
    validate_trace_rule_name("trace stop rule", trace_options.stop_after.as_deref())?;

    if detect_bundle(source, &options.filename)?.is_some() {
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

pub fn unpack(source: &str, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack");
    let _enter = span.enter();

    match detect_bundle(source, &options.filename)? {
        Some(result) => unpack_multi_module(result.modules, options),
        None if options.heuristic_split => match scope_hoist::split_scope_hoisted(source) {
            Some(result) if result.modules.len() > 1 => {
                let mut opts = options.clone();
                opts.dead_code_elimination = false;
                unpack_multi_module(result.modules, opts)
            }
            _ => {
                let output = decompile(source, options)?;
                Ok(UnpackOutput {
                    modules: vec![("module.js".to_string(), output.code)],
                    warnings: output.warnings,
                })
            }
        },
        None => {
            let output = decompile(source, options)?;
            Ok(UnpackOutput {
                modules: vec![("module.js".to_string(), output.code)],
                warnings: output.warnings,
            })
        }
    }
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns the module code exactly as produced by the bundle detector.
/// Some detectors still do minimal runtime normalization during extraction so
/// their output can be parsed as standalone modules, but cross-module analysis
/// and the normal rule pipeline are skipped.
///
/// Like [`unpack_multi_module`], individual module parse failures fall back to
/// raw code and are reported via `UnpackOutput::warnings`.
pub fn unpack_raw(source: &str, options: &DecompileOptions) -> Result<UnpackOutput> {
    let result = detect_bundle(source, &options.filename)?.or_else(|| {
        if options.heuristic_split {
            let r = scope_hoist::split_scope_hoisted(source)?;
            if r.modules.len() > 1 {
                Some(r)
            } else {
                None
            }
        } else {
            None
        }
    });
    match result {
        Some(result) => {
            let mut warnings = Vec::new();
            let modules = result
                .modules
                .into_iter()
                .map(|module| {
                    let code = match normalize_raw_unpacked_module(&module.code, &module.filename) {
                        Ok(normalized) => normalized,
                        Err(e) => {
                            warnings.push(UnpackWarning::new(
                                module.filename.clone(),
                                UnpackWarningKind::RawNormalizationFailed,
                                format!("raw normalization failed, preserving unparsed code: {e}"),
                            ));
                            module.code
                        }
                    };
                    (module.filename, code)
                })
                .collect();
            Ok(UnpackOutput { modules, warnings })
        }
        None => Ok(UnpackOutput {
            modules: vec![("module.js".to_string(), source.to_string())],
            warnings: Vec::new(),
        }),
    }
}

fn detect_bundle(source: &str, filename: &str) -> Result<Option<UnpackResult>> {
    let span = tracing::info_span!("detect_bundle");
    let _enter = span.enter();

    match try_unpack_bundle(source) {
        Ok(result) => Ok(result),
        Err(bundle_parse_error) => {
            // Bundle detection intentionally parses only ES/JSX. Preserve the
            // single-file fallback for valid inputs that use filename-driven
            // syntax such as TypeScript. That means a second parse here is
            // intentional: it distinguishes unsupported bundle syntax from
            // genuinely invalid input.
            let input_parse_result = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                parse_js(source, filename, cm)
            });
            match input_parse_result {
                Ok(_) => Ok(None),
                Err(input_parse_error) => Err(anyhow!(
                    "{input_parse_error}; bundle detection also failed: {bundle_parse_error}"
                )),
            }
        }
    }
}

fn normalize_raw_unpacked_module(source: &str, filename: &str) -> Result<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, filename, cm.clone())?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module.visit_mut_with(&mut UnEsm::new(unresolved_mark, RewriteLevel::Standard));
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm)
    })
}

/// Multi-module unpack with cross-module late pass.
///
/// Phase 1: parse + Stage 1+2 + collect facts (facts only, code discarded)
/// Phase 2: full pipeline from scratch with late pass injected at barrier
///
/// Stage 1+2 runs twice per module — once for fact collection, once for the real pipeline.
/// This is necessary because SWC's SyntaxContext must remain continuous across the entire
/// pipeline (re-parsing creates fresh contexts that break rename rules).
///
/// # Best-effort semantics
///
/// Individual extracted modules that fail to parse are preserved as raw code
/// rather than aborting the entire unpack. The extraction process can
/// produce module bodies that are not valid standalone JS (e.g. incomplete
/// slicing, runtime wrapper residue). Hard-failing on those would discard
/// all other successfully extracted modules, which is worse for both
/// interactive and automated users. Failures are reported via
/// `UnpackOutput::warnings` so callers can surface them without silent
/// swallowing.
///
/// Both phases run via rayon. On targets without threading support, Rayon falls
/// back to sequential execution.
fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack_multi_module", count = modules.len());
    let _enter = span.enter();

    // Parse the sourcemap once before the loop.
    let parsed_sourcemap = options
        .sourcemap
        .as_deref()
        .map(parse_sourcemap)
        .transpose()?;

    // Phase 1: collect facts. Run Stage 1+2 on each module and extract
    // import/export facts. The AST is discarded — only facts survive the barrier.
    let collect_facts =
        |unpacked: &crate::unpacker::UnpackedModule| -> (
            String,
            crate::facts::ModuleFacts,
            Option<UnpackWarning>,
        ) {
            let (facts, warning) = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module = match parse_js(&unpacked.code, &unpacked.filename, cm) {
                    Ok(module) => module,
                    Err(e) => {
                        return (
                            crate::facts::ModuleFacts::default(),
                            Some(UnpackWarning::new(
                                unpacked.filename.clone(),
                                UnpackWarningKind::FactCollectionParseFailed,
                                format!("parse failed during fact collection, using empty facts: {e}"),
                            )),
                        );
                    }
                };
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_rules_until(&mut module, unresolved_mark, "UnEsm");
                (collect_module_facts(&module), None)
            });
            (unpacked.filename.clone(), facts, warning)
        };

    let phase1: Vec<_> = {
        let span = tracing::info_span!("phase1_collect_facts");
        let _enter = span.enter();
        modules.par_iter().map(collect_facts).collect()
    };

    let mut module_facts = ModuleFactsMap::new();
    let mut warnings = Vec::new();
    for (filename, facts, warning) in phase1 {
        module_facts.insert(&filename, facts);
        if let Some(w) = warning {
            warnings.push(w);
        }
    }

    // Phase 2: full pipeline with late pass. Each module runs the entire
    // pipeline from scratch. Between Stage 2 and Stage 3, the late pass applies
    // cross-module rewrites using the facts collected in Phase 1.
    let facts_ref = &module_facts;
    let sm_ref = &parsed_sourcemap;

    let decompile_module =
        |unpacked: crate::unpacker::UnpackedModule| -> (String, String, Vec<UnpackWarning>) {
            match GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let parsed =
                    parse_js_with_recovery(&unpacked.code, &unpacked.filename, cm.clone())?;
                let mut module = parsed.module;
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
                if let Some(sm) = sm_ref {
                    module.visit_mut_with(&mut ImportDedup);
                    apply_sourcemap_renames(&mut module, sm, &cm, unresolved_mark);
                    module.visit_mut_with(&mut UnImportRename);
                }

                let mut diag_warnings = if options.diagnostics {
                    let mut warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
                    warnings.extend(collect_tdz_warnings(&module, &unpacked.filename));
                    warnings.extend(collect_duplicate_declaration_warnings(
                        &module,
                        &unpacked.filename,
                    ));
                    warnings
                } else {
                    Vec::new()
                };

                module.visit_mut_with(&mut fixer(None));
                let code = print_js(&module, cm)?;

                if options.diagnostics {
                    diag_warnings.extend(verify_output_parses(&code, &unpacked.filename));
                }

                Ok::<_, anyhow::Error>((code, diag_warnings))
            }) {
                Ok((code, diag_warnings)) => (unpacked.filename, code, diag_warnings),
                Err(e) => (
                    unpacked.filename.clone(),
                    unpacked.code,
                    vec![UnpackWarning::new(
                        unpacked.filename,
                        UnpackWarningKind::DecompileFailed,
                        format!("decompile failed, preserving raw code: {e}"),
                    )],
                ),
            }
        };

    let triples: Vec<_> = {
        let span = tracing::info_span!("phase2_decompile_modules");
        let _enter = span.enter();
        modules.into_par_iter().map(decompile_module).collect()
    };

    let mut modules = Vec::with_capacity(triples.len());
    for (filename, code, module_warnings) in triples {
        modules.push((filename, code));
        warnings.extend(module_warnings);
    }

    Ok(UnpackOutput { modules, warnings })
}

#[derive(Debug, Clone)]
struct ParsedModule {
    module: Module,
    recoverable_errors: Vec<ParseDiagnostic>,
}

#[derive(Debug, Clone)]
struct ParseDiagnostic {
    filename: String,
    line: usize,
    column: usize,
    message: String,
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

pub(crate) fn parse_js(source: &str, filename: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    Ok(parse_js_with_recovery(source, filename, cm)?.module)
}

fn parse_js_with_recovery(
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
    let parsed = parser.parse_module();
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

pub(crate) fn print_js(module: &Module, cm: Lrc<SourceMap>) -> Result<String> {
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

fn collect_tdz_warnings(module: &Module, filename: &str) -> Vec<UnpackWarning> {
    crate::tdz_check::check_tdz(module)
        .into_iter()
        .map(|v| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::TdzViolation,
                format!("reference to `{}` before declaration", v.name),
            )
        })
        .collect()
}

fn collect_input_parse_warnings(errors: &[ParseDiagnostic]) -> Vec<UnpackWarning> {
    errors
        .iter()
        .map(|error| {
            UnpackWarning::new(
                &error.filename,
                UnpackWarningKind::InputParseRecovered,
                format!("input parse recovered from parser error: {error}"),
            )
        })
        .collect()
}

fn collect_duplicate_declaration_warnings(module: &Module, filename: &str) -> Vec<UnpackWarning> {
    let mut collector = DuplicateDeclarationCollector::default();
    module.visit_with(&mut collector);
    collector
        .duplicates
        .into_iter()
        .map(|name| {
            UnpackWarning::new(
                filename,
                UnpackWarningKind::DuplicateDeclaration,
                format!("duplicate lexical declaration `{name}`"),
            )
        })
        .collect()
}

#[derive(Default)]
struct DuplicateDeclarationCollector {
    seen: HashMap<(Atom, swc_core::common::SyntaxContext), ()>,
    duplicates: Vec<Atom>,
}

impl DuplicateDeclarationCollector {
    fn record_binding(&mut self, binding: &BindingIdent) {
        let key = (binding.id.sym.clone(), binding.id.ctxt);
        if self.seen.insert(key, ()).is_some() && !self.duplicates.contains(&binding.id.sym) {
            self.duplicates.push(binding.id.sym.clone());
        }
    }

    fn record_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.record_binding(binding),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.record_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(kv) => self.record_pat(&kv.value),
                        ObjectPatProp::Assign(assign) => {
                            self.record_binding(&assign.key);
                        }
                        ObjectPatProp::Rest(rest) => self.record_pat(&rest.arg),
                    }
                }
            }
            Pat::Rest(rest) => self.record_pat(&rest.arg),
            Pat::Assign(assign) => self.record_pat(&assign.left),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }
}

impl Visit for DuplicateDeclarationCollector {
    fn visit_class_decl(&mut self, class_decl: &ClassDecl) {
        self.record_binding(&BindingIdent {
            id: class_decl.ident.clone(),
            type_ann: None,
        });
        class_decl.class.visit_with(self);
    }

    fn visit_import_decl(&mut self, import_decl: &ImportDecl) {
        for specifier in &import_decl.specifiers {
            match specifier {
                ImportSpecifier::Named(named) => self.record_binding(&BindingIdent {
                    id: named.local.clone(),
                    type_ann: None,
                }),
                ImportSpecifier::Default(default) => self.record_binding(&BindingIdent {
                    id: default.local.clone(),
                    type_ann: None,
                }),
                ImportSpecifier::Namespace(namespace) => self.record_binding(&BindingIdent {
                    id: namespace.local.clone(),
                    type_ann: None,
                }),
            }
        }
    }

    fn visit_var_decl(&mut self, var_decl: &VarDecl) {
        if var_decl.kind == VarDeclKind::Var {
            return;
        }
        for decl in &var_decl.decls {
            self.record_pat(&decl.name);
        }
        var_decl.visit_children_with(self);
    }

    fn visit_block_stmt(&mut self, block: &swc_core::ecma::ast::BlockStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        block.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_function(&mut self, func: &swc_core::ecma::ast::Function) {
        let mut child = DuplicateDeclarationCollector::default();
        func.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let mut child = DuplicateDeclarationCollector::default();
        arrow.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_class(&mut self, class: &swc_core::ecma::ast::Class) {
        let mut child = DuplicateDeclarationCollector::default();
        class.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        let mut child = DuplicateDeclarationCollector::default();
        stmt.visit_children_with(&mut child);
        self.duplicates.extend(child.duplicates);
    }
}

fn verify_output_parses(code: &str, filename: &str) -> Vec<UnpackWarning> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        match parse_js_with_recovery(code, filename, cm) {
            Ok(parsed) => parsed
                .recoverable_errors
                .into_iter()
                .map(|error| {
                    UnpackWarning::new(
                        filename,
                        UnpackWarningKind::OutputParseRecovered,
                        format!("emitted output parse recovered from parser error: {error}"),
                    )
                })
                .collect(),
            Err(e) => vec![UnpackWarning::new(
                filename,
                UnpackWarningKind::OutputParseFailed,
                format!("emitted output failed to parse: {e}"),
            )],
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpacker::UnpackedModule;

    #[test]
    fn parse_js_reports_parse_errors_after_parsing() {
        let cm: Lrc<SourceMap> = Default::default();
        let err = parse_js("const = ;", "broken.js", cm).expect_err("invalid JS should fail");

        assert!(
            err.to_string().contains("broken.js"),
            "error should include filename: {err}"
        );
    }

    #[test]
    fn unpack_raw_preserves_unparseable_extracted_modules() {
        let result = unpack_raw(
            "const = ;",
            &DecompileOptions {
                heuristic_split: false,
                ..Default::default()
            },
        );

        assert!(result.is_err(), "invalid top-level input should still fail");

        let modules = vec![UnpackedModule {
            id: "1".to_string(),
            is_entry: false,
            code: "const = ;".to_string(),
            filename: "module-1.js".to_string(),
        }];
        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("unparseable extracted modules should be preserved as raw code");
        assert_eq!(
            output.modules,
            vec![("module-1.js".to_string(), "const = ;".to_string())]
        );
        assert!(
            !output.warnings.is_empty(),
            "should warn about unparseable module"
        );
        let warning_kinds = output
            .warnings
            .iter()
            .map(|warning| {
                assert_eq!(warning.filename, "module-1.js");
                warning.kind
            })
            .collect::<Vec<_>>();
        assert_eq!(
            warning_kinds,
            vec![
                UnpackWarningKind::FactCollectionParseFailed,
                UnpackWarningKind::DecompileFailed
            ]
        );
    }

    #[test]
    fn unpack_propagates_invalid_input_parse_errors() {
        let err = unpack(
            "const = ;",
            DecompileOptions {
                filename: "broken.js".to_string(),
                ..Default::default()
            },
        )
        .expect_err("invalid source should fail");

        assert!(
            err.to_string().contains("broken.js"),
            "error should include input filename: {err}"
        );
    }

    #[test]
    fn unpack_preserves_typescript_single_file_fallback() {
        let output = unpack(
            "const value: number = 1;",
            DecompileOptions {
                filename: "input.ts".to_string(),
                ..Default::default()
            },
        )
        .expect("valid TypeScript should fall back to single-file decompile");

        assert_eq!(output.modules.len(), 1);
        assert_eq!(output.modules[0].0, "module.js");
        assert!(
            output.modules[0].1.contains("const value"),
            "expected TypeScript input to decompile, got: {}",
            output.modules[0].1
        );
    }

    #[test]
    fn diagnostics_off_by_default() {
        let output = decompile("console.log(x);\nlet x = 1;", DecompileOptions::default())
            .expect("decompile should succeed");
        assert!(
            output.warnings.is_empty(),
            "default decompile should produce no diagnostic warnings"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_tdz() {
        let output = decompile(
            "console.log(x);\nlet x = 1;",
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::TdzViolation),
            "diagnostics should report TDZ violation"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_recovered_input_parse_errors() {
        let output = decompile(
            "label: label: break label;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate-label.js".to_string(),
                ..Default::default()
            },
        )
        .expect("recovered input parse should still decompile");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::InputParseRecovered),
            "diagnostics should report recovered input parse error: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_recovered_output_parse_errors() {
        let output = decompile(
            "label: label: break label;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate-label.js".to_string(),
                ..Default::default()
            },
        )
        .expect("recovered output parse should still return emitted code");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::OutputParseRecovered),
            "diagnostics should report recovered output parse error: {:?}",
            output.warnings
        );
        assert!(
            output.has_errors(),
            "recovered output parse errors should be error-severity diagnostics"
        );
    }

    #[test]
    fn diagnostics_opt_in_reports_duplicate_lexical_declarations() {
        let output = decompile(
            "let a = 1;\nlet a = 2;",
            DecompileOptions {
                diagnostics: true,
                filename: "duplicate.js".to_string(),
                ..Default::default()
            },
        )
        .expect("duplicate declarations should still return emitted code");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "diagnostics should report duplicate declaration: {:?}",
            output.warnings
        );
        assert!(
            output.has_errors(),
            "duplicate declarations should be error-severity diagnostics"
        );
    }

    #[test]
    fn diagnostics_duplicate_declarations_inside_block() {
        let output = decompile(
            "{ const x = 1; const x = 2; }",
            DecompileOptions {
                diagnostics: true,
                filename: "block-dup.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "should detect duplicate declarations inside block: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_no_false_positive_across_block_scopes() {
        let output = decompile(
            "{ const x = 1; } { const x = 2; }",
            DecompileOptions {
                diagnostics: true,
                filename: "separate-scopes.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "separate block scopes should not report duplicate: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_no_false_positive_across_for_of_loops() {
        let output = decompile(
            "function f(items) { for (const x of items) { } for (const x of items) { } }",
            DecompileOptions {
                diagnostics: true,
                filename: "for-of-scopes.js".to_string(),
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output
                .warnings
                .iter()
                .any(|w| w.kind == UnpackWarningKind::DuplicateDeclaration),
            "separate for-of loop scopes should not report duplicate: {:?}",
            output.warnings
        );
    }

    #[test]
    fn diagnostics_valid_output_no_parse_warning() {
        let output = decompile(
            "const x = 1;",
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("decompile should succeed");
        assert!(
            !output.warnings.iter().any(|w| matches!(
                w.kind,
                UnpackWarningKind::OutputParseRecovered | UnpackWarningKind::OutputParseFailed
            )),
            "valid output should not produce parse warning"
        );
    }
}
