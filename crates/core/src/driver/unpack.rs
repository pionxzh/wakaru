use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    CallExpr, Callee, Expr, ExprOrSpread, Lit, MemberExpr, MemberProp, Module, Str,
};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::diagnostics::{
    collect_duplicate_declaration_warnings, collect_input_parse_warnings, collect_tdz_warnings,
    verify_output_parses,
};
use super::io::{parse_js, parse_js_with_recovery, print_js};
use super::single_file::decompile;
use super::types::{DecompileOptions, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind};
use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::babel_helper_utils::LocalHelperContext;
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, ImportDedup, RewriteLevel, RulePipelineOptions,
    SimplifySequence, UnAssignmentMerging, UnConditionalsAssignmentOnly,
    UnConditionalsExprStmtOnly, UnEsm, UnExportRename, UnIife, UnImportRename, UnObjectSpread,
    UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::unpacker::{scope_hoist, try_unpack_bundle, webpack5, UnpackResult, UnpackedModule};
use crate::utils::paren::{strip_parens, strip_parens_mut};

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

pub fn unpack_files(
    mut inputs: Vec<UnpackInput>,
    mut options: DecompileOptions,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one input file is required"));
    }

    if inputs.len() == 1 {
        let input = inputs.pop().expect("checked input length");
        options.filename = input.filename;
        return unpack(&input.source, options);
    }

    let span = tracing::info_span!("unpack_files", count = inputs.len());
    let _enter = span.enter();

    let mut modules = Vec::new();
    for input in inputs {
        match detect_bundle(&input.source, &input.filename)? {
            Some(result) => {
                let chunk_ids = webpack5::detect_chunk_ids(&input.source);
                let input_filename = input.filename.clone();
                modules.extend(result.modules.into_iter().map(|module| {
                    MultiSourceModule::detected(module, chunk_ids.clone(), input_filename.clone())
                }))
            }
            None if options.heuristic_split => {
                match scope_hoist::split_scope_hoisted(&input.source) {
                    Some(result) if result.modules.len() > 1 => {
                        modules.extend(result.modules.into_iter().map(MultiSourceModule::fallback))
                    }
                    _ => modules.push(MultiSourceModule::fallback(
                        crate::unpacker::UnpackedModule {
                            id: input.filename.clone(),
                            is_entry: false,
                            code: input.source,
                            filename: filename_for_fallback_input(&input.filename),
                        },
                    )),
                }
            }
            None => modules.push(MultiSourceModule::fallback(
                crate::unpacker::UnpackedModule {
                    id: input.filename.clone(),
                    is_entry: false,
                    code: input.source,
                    filename: filename_for_fallback_input(&input.filename),
                },
            )),
        }
    }

    if modules.is_empty() {
        return Err(anyhow!("no modules were extracted from input files"));
    }

    let modules = stabilize_multi_source_modules(modules);
    unpack_multi_module(modules, options)
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns raw module output after detector-specific extraction and raw
/// ESM/runtime normalization. Cross-module analysis and the normal decompile
/// rule pipeline are skipped.
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

pub fn unpack_files_raw(
    mut inputs: Vec<UnpackInput>,
    options: &DecompileOptions,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one input file is required"));
    }

    if inputs.len() == 1 {
        let input = inputs.pop().expect("checked input length");
        let mut opts = options.clone();
        opts.filename = input.filename;
        return unpack_raw(&input.source, &opts);
    }

    let mut warnings = Vec::new();
    let mut modules = Vec::new();

    for input in inputs {
        let result = detect_bundle(&input.source, &input.filename)?.or_else(|| {
            if options.heuristic_split {
                let r = scope_hoist::split_scope_hoisted(&input.source)?;
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
                for module in result.modules {
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
                    modules.push((module.filename, code));
                }
            }
            None => modules.push((
                filename_for_fallback_input(&input.filename),
                input.source.to_string(),
            )),
        }
    }

    Ok(UnpackOutput { modules, warnings })
}

fn filename_for_fallback_input(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("module.js")
        .to_string()
}

struct MultiSourceModule {
    module: UnpackedModule,
    allow_cross_chunk_rewrite: bool,
    chunk_ids: HashSet<usize>,
    input_filename: String,
    input_group: String,
}

impl MultiSourceModule {
    fn detected(module: UnpackedModule, chunk_ids: HashSet<usize>, input_filename: String) -> Self {
        let input_group = input_group_for_filename(&input_filename);
        Self {
            module,
            allow_cross_chunk_rewrite: true,
            chunk_ids,
            input_filename,
            input_group,
        }
    }

    fn fallback(module: UnpackedModule) -> Self {
        Self {
            module,
            allow_cross_chunk_rewrite: false,
            chunk_ids: HashSet::new(),
            input_filename: String::new(),
            input_group: String::new(),
        }
    }
}

fn stabilize_multi_source_modules(mut modules: Vec<MultiSourceModule>) -> Vec<UnpackedModule> {
    assign_unique_module_filenames(&mut modules);
    let id_to_filename = unique_numeric_chunk_module_id_map(&modules);

    if !id_to_filename.is_empty() {
        for module in &mut modules {
            if !module.allow_cross_chunk_rewrite {
                continue;
            }
            if let Ok(code) = rewrite_webpack_numeric_module_refs(
                &module.module.code,
                &module.module.filename,
                &module.input_group,
                &id_to_filename,
            ) {
                module.module.code = code;
            }
        }
    }

    modules.into_iter().map(|module| module.module).collect()
}

fn assign_unique_module_filenames(modules: &mut [MultiSourceModule]) {
    let mut seen = HashSet::new();
    for module in modules {
        module.module.filename = deduplicate_module_filename(&module.module.filename, &mut seen);
    }
}

fn deduplicate_module_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    let key = filename.to_lowercase();
    if seen.insert(key) {
        return filename.to_string();
    }

    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("js");
    let parent = path.parent().unwrap_or(std::path::Path::new(""));
    let mut n = 2u32;
    loop {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        let candidate = candidate.to_string_lossy().replace('\\', "/");
        let candidate_key = candidate.to_lowercase();
        if seen.insert(candidate_key) {
            return candidate;
        }
        n += 1;
    }
}

fn input_group_for_filename(filename: &str) -> String {
    let parent = std::path::Path::new(filename)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    normalize_input_group_path(parent)
}

fn normalize_input_group_path(path: &std::path::Path) -> String {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    normalize_path_lexically(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_path_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn chunk_filename_matches_id(filename: &str, chunk_id: usize) -> bool {
    let Some(name) = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    name == format!("{chunk_id}.js") || name == format!("{chunk_id}.bundle.js")
}

fn unique_numeric_chunk_module_id_map(
    modules: &[MultiSourceModule],
) -> HashMap<(String, usize, usize), String> {
    let mut counts: HashMap<(String, usize, usize), (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite || module.chunk_ids.is_empty() {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        for chunk_id in &module.chunk_ids {
            if !chunk_filename_matches_id(&module.input_filename, *chunk_id) {
                continue;
            }
            let entry = counts
                .entry((module.input_group.clone(), *chunk_id, id))
                .or_insert((0, module.module.filename.clone()));
            entry.0 += 1;
            entry.1 = module.module.filename.clone();
        }
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
}

fn rewrite_webpack_numeric_module_refs(
    source: &str,
    filename: &str,
    input_group: &str,
    id_to_filename: &HashMap<(String, usize, usize), String>,
) -> Result<String> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, filename, cm.clone())?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        module.visit_mut_with(&mut WebpackNumericReferenceRewriter {
            input_group,
            id_to_filename,
        });
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm)
    })
}

struct WebpackNumericReferenceRewriter<'a> {
    input_group: &'a str,
    id_to_filename: &'a HashMap<(String, usize, usize), String>,
}

impl VisitMut for WebpackNumericReferenceRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        self.rewrite_async_chunk_t_bind(call);
        call.visit_mut_children_with(self);
    }
}

impl WebpackNumericReferenceRewriter<'_> {
    fn rewrite_async_chunk_t_bind(&self, call: &mut CallExpr) {
        let Some((runtime, chunk_id)) = self.extract_then_chunk_loader(&call.callee) else {
            return;
        };
        let Some(arg) = call.args.first_mut() else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Expr::Call(bind_call) = strip_parens_mut(&mut arg.expr) else {
            return;
        };
        self.rewrite_t_bind_module_arg(bind_call, &runtime, chunk_id);
    }

    fn extract_then_chunk_loader(&self, callee: &Callee) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, "then") {
            return None;
        }

        let Expr::Call(load_call) = strip_parens(obj) else {
            return None;
        };
        self.extract_runtime_member_numeric_arg(load_call, "e", 0)
    }

    fn rewrite_t_bind_module_arg(
        &self,
        call: &mut CallExpr,
        expected_runtime: &RuntimeIdent,
        chunk_id: usize,
    ) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return;
        };
        if !member_prop_is(prop, "bind") {
            return;
        }
        let Some(runtime) = self.extract_runtime_t_member(obj) else {
            return;
        };
        if &runtime != expected_runtime {
            return;
        }

        let Some(this_arg) = call.args.first() else {
            return;
        };
        if this_arg.spread.is_some() {
            return;
        }
        let Expr::Ident(this_ident) = strip_parens(&this_arg.expr) else {
            return;
        };
        if runtime != RuntimeIdent::from_ident(this_ident) {
            return;
        }

        self.rewrite_chunk_module_arg(&mut call.args, 1, chunk_id);
    }

    fn extract_runtime_member_numeric_arg(
        &self,
        call: &CallExpr,
        expected_prop: &str,
        arg_index: usize,
    ) -> Option<(RuntimeIdent, usize)> {
        let Callee::Expr(callee_expr) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) else {
            return None;
        };
        if !member_prop_is(prop, expected_prop) {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        let arg = call.args.get(arg_index)?;
        if arg.spread.is_some() {
            return None;
        }
        let module_id = numeric_arg_id(&arg.expr)?;
        Some((RuntimeIdent::from_ident(runtime), module_id))
    }

    fn extract_runtime_t_member(&self, expr: &Expr) -> Option<RuntimeIdent> {
        let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(expr) else {
            return None;
        };
        if !member_prop_is(prop, "t") {
            return None;
        }
        let Expr::Ident(runtime) = strip_parens(obj) else {
            return None;
        };
        Some(RuntimeIdent::from_ident(runtime))
    }

    fn rewrite_chunk_module_arg(&self, args: &mut [ExprOrSpread], index: usize, chunk_id: usize) {
        let Some(arg) = args.get_mut(index) else {
            return;
        };
        if arg.spread.is_some() {
            return;
        }
        let Some(module_id) = numeric_arg_id(&arg.expr) else {
            return;
        };
        let Some(filename) =
            self.id_to_filename
                .get(&(self.input_group.to_string(), chunk_id, module_id))
        else {
            return;
        };
        let path = format!("./{filename}");
        *arg.expr = Expr::Lit(Lit::Str(Str {
            span: DUMMY_SP,
            value: path.into(),
            raw: None,
        }));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeIdent {
    sym: Atom,
    ctxt: SyntaxContext,
}

impl RuntimeIdent {
    fn from_ident(ident: &swc_core::ecma::ast::Ident) -> Self {
        Self {
            sym: ident.sym.clone(),
            ctxt: ident.ctxt,
        }
    }
}

fn numeric_arg_id(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = strip_parens(expr) else {
        return None;
    };
    let value = number.value;
    if value < 0.0 || value.fract() != 0.0 {
        return None;
    }
    Some(value as usize)
}

fn member_prop_is(prop: &MemberProp, expected: &str) -> bool {
    matches!(prop, MemberProp::Ident(ident) if ident.sym.as_ref() == expected)
}

pub(super) fn detect_bundle(source: &str, filename: &str) -> Result<Option<UnpackResult>> {
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
        recover_late_esm_from_factory_iifes(&mut module, unresolved_mark, RewriteLevel::Standard);
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm)
    })
}

fn recover_late_esm_from_factory_iifes(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
) {
    module.visit_mut_with(&mut ArrowFunction);
    module.visit_mut_with(&mut ArrowReturn);
    module.visit_mut_with(&mut UnIife::new(level));
    apply_rules(
        module,
        unresolved_mark,
        RulePipelineOptions::between("UnCurlyBraces", "UnEsm").with_rewrite_level(level),
    );
    module.visit_mut_with(&mut UnExportRename);
    module.visit_mut_with(&mut ArrowReturn);
}

/// Multi-module unpack with cross-module late pass.
///
/// Phase 1: parse + through-UnEsm range + ESM recovery + collect facts (code discarded)
/// Phase 2: parse + through-UnEsm range + late pass + UnTemplateLiteral-through-UnReturn range
///
/// The through-UnEsm range runs twice per module — once for fact collection, once
/// for the real output pipeline. This is necessary because SWC's SyntaxContext
/// must remain continuous within the emitted module pipeline; reusing a Phase 1
/// AST after a separate parse would break rename rules.
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

    // Phase 1: collect facts. Run the through-UnEsm normalization range on each
    // module and extract import/export facts. The AST is discarded — only facts
    // survive the barrier.
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
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
                );
                recover_late_esm_from_factory_iifes(
                    &mut module,
                    unresolved_mark,
                    RewriteLevel::Standard,
                );
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

    // Phase 2: output pipeline with late pass. Each module is parsed from
    // scratch, runs the same through-UnEsm range, then crosses the facts barrier
    // before the remaining rule range and targeted late cleanup.
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

                // Through-UnEsm range.
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
                );

                // Late pass at the barrier
                run_reexport_consolidation(&mut module, facts_ref);
                run_namespace_decomposition(&mut module, facts_ref);
                let local_helpers = LocalHelperContext::collect(&module);
                UnObjectSpread::run_with_helpers(&mut module, &local_helpers, Some(facts_ref));

                // UnTemplateLiteral-through-UnReturn range.
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::between("UnTemplateLiteral", "UnReturn")
                        .with_dead_code_elimination(options.dead_code_elimination)
                        .with_rewrite_level(options.level)
                        .with_module_facts(facts_ref),
                );
                // Later rules can expose sequence expressions. The old unpack
                // path cleaned those by running a second full module pipeline;
                // keep only the syntax cleanup needed after the split.
                module.visit_mut_with(&mut SimplifySequence::new_with_level(
                    unresolved_mark,
                    options.level,
                ));
                module.visit_mut_with(&mut UnAssignmentMerging);
                // UnIife2 can expose webpack export helpers that were hidden in
                // factory wrappers at the Stage 2 barrier. Recover just that ESM
                // shape without restoring the old full second pass.
                recover_late_esm_from_factory_iifes(&mut module, unresolved_mark, options.level);
                module.visit_mut_with(&mut UnOptionalChaining::new(unresolved_mark, options.level));
                module.visit_mut_with(&mut UnConditionalsAssignmentOnly);
                module.visit_mut_with(&mut UnConditionalsExprStmtOnly);

                // Source-map-enhanced passes
                if let Some(sm) = sm_ref {
                    module.visit_mut_with(&mut ImportDedup);
                    apply_sourcemap_renames(&mut module, sm, &cm, unresolved_mark);
                    module.visit_mut_with(&mut UnImportRename::new(unresolved_mark));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpacker::UnpackedModule;

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
}
