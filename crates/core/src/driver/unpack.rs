use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
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
use super::types::{
    DecompileOptions, ModuleProvenance, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind,
};
#[cfg(test)]
use super::unpack_cleanup::hoist_late_runtime_helpers;
use super::unpack_cleanup::{dedup_duplicate_exports, prune_stale_local_named_exports};
use super::unpack_cycles::{collect_import_cycle_warnings, merge_import_cycles};
#[cfg(test)]
use super::unpack_cycles::{scan_local_import_dependencies, unsafe_merge_member_reason};
use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, ImportDedup, RewriteLevel, RulePipelineOptions,
    SimplifySequence, SmartRename, UnAssignmentMerging, UnConditionals,
    UnConditionalsAssignmentOnly, UnEsm, UnExportRename, UnIife, UnImportRename,
    UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::unpacker::{scope_hoist, try_unpack_bundle, webpack5, UnpackResult, UnpackedModule};
use crate::utils::paren::{strip_parens, strip_parens_mut};

pub fn unpack(source: &str, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack");
    let _enter = span.enter();

    match detect_bundle(source, &options.filename)? {
        Some(result) => unpack_unpack_result(result, options),
        None if options.heuristic_split => match scope_hoist::split_scope_hoisted(source) {
            Some(result) if result.modules.len() > 1 => {
                let mut opts = options.clone();
                opts.dead_code_elimination = false;
                unpack_unpack_result(result, opts)
            }
            _ => {
                let output = decompile(source, options)?;
                Ok(UnpackOutput {
                    modules: vec![("module.js".to_string(), output.code)],
                    provenance: vec![whole_input_provenance("module.js", "", source)],
                    warnings: output.warnings,
                })
            }
        },
        None => {
            let output = decompile(source, options)?;
            Ok(UnpackOutput {
                modules: vec![("module.js".to_string(), output.code)],
                provenance: vec![whole_input_provenance("module.js", "", source)],
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
                let allow_cycle_premerge = result.allow_cycle_premerge;
                modules.extend(result.modules.into_iter().map(|module| {
                    MultiSourceModule::detected(
                        module,
                        chunk_ids.clone(),
                        input_filename.clone(),
                        allow_cycle_premerge,
                    )
                }))
            }
            None if options.heuristic_split => {
                match scope_hoist::split_scope_hoisted(&input.source) {
                    Some(result) if result.modules.len() > 1 => {
                        modules.extend(result.modules.into_iter().map(|mut module| {
                            module.source_input = input.filename.clone();
                            MultiSourceModule::fallback(module)
                        }))
                    }
                    _ => modules.push(MultiSourceModule::fallback(
                        crate::unpacker::UnpackedModule {
                            id: input.filename.clone(),
                            is_entry: false,
                            filename: filename_for_fallback_input(&input.filename),
                            source_ranges: vec![(0, input.source.len() as u32)],
                            source_input: input.filename.clone(),
                            code: input.source,
                        },
                    )),
                }
            }
            None => modules.push(MultiSourceModule::fallback(
                crate::unpacker::UnpackedModule {
                    id: input.filename.clone(),
                    is_entry: false,
                    filename: filename_for_fallback_input(&input.filename),
                    source_ranges: vec![(0, input.source.len() as u32)],
                    source_input: input.filename.clone(),
                    code: input.source,
                },
            )),
        }
    }

    if modules.is_empty() {
        return Err(anyhow!("no modules were extracted from input files"));
    }

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    unpack_multi_module_with_plan(modules, numeric_rewrite_plan, options)
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns raw module output after detector-specific extraction and
/// bundler-coupled normalization. Cross-module analysis and the normal
/// decompile rule pipeline are skipped.
pub fn unpack_raw(source: &str, options: &DecompileOptions) -> Result<UnpackOutput> {
    let result = detect_bundle_raw(source, &options.filename)?
        .map(|result| (result, false))
        .or_else(|| {
            if options.heuristic_split {
                let r = scope_hoist::split_scope_hoisted(source)?;
                if r.modules.len() > 1 {
                    Some((r, true))
                } else {
                    None
                }
            } else {
                None
            }
        });
    match result {
        Some((result, normalize_for_runnable_split)) => {
            let (modules, provenance, warnings) = if normalize_for_runnable_split {
                // Heuristic scope-hoisted fallback does not get the esbuild
                // detector's bundler-specific cleanup, so keep the narrow
                // runnable normalization it still relies on.
                let (modules, warnings) = {
                    if should_merge_raw_import_cycles(&result.modules) {
                        let span = tracing::info_span!("raw_merge_import_cycles");
                        let _enter = span.enter();
                        merge_import_cycles(result.modules)
                    } else {
                        (result.modules, Vec::new())
                    }
                };
                let provenance = module_provenance(&modules);
                let normalized: Vec<_> = modules
                    .into_par_iter()
                    .map(|module| {
                        match normalize_raw_unpacked_module(&module.code, &module.filename) {
                            Ok(normalized) => ((module.filename, normalized), None),
                            Err(e) => {
                                let warning = UnpackWarning::new(
                                    module.filename.clone(),
                                    UnpackWarningKind::RawNormalizationFailed,
                                    format!(
                                        "raw normalization failed, preserving unparsed code: {e}"
                                    ),
                                );
                                ((module.filename, module.code), Some(warning))
                            }
                        }
                    })
                    .collect();
                let mut output_modules = Vec::with_capacity(normalized.len());
                let mut output_warnings = if options.diagnostics {
                    warnings
                } else {
                    Vec::new()
                };
                for (module, warning) in normalized {
                    if options.diagnostics {
                        if let Some(warning) = warning {
                            output_warnings.push(warning);
                        }
                    }
                    output_modules.push(module);
                }
                (output_modules, provenance, output_warnings)
            } else {
                let provenance = module_provenance(&result.modules);
                (
                    result
                        .modules
                        .into_iter()
                        .map(|module| (module.filename, module.code))
                        .collect(),
                    provenance,
                    Vec::new(),
                )
            };
            Ok(UnpackOutput {
                modules,
                provenance,
                warnings,
            })
        }
        None => Ok(UnpackOutput {
            modules: vec![("module.js".to_string(), source.to_string())],
            provenance: vec![whole_input_provenance("module.js", "", source)],
            warnings: Vec::new(),
        }),
    }
}

/// Provenance entries for a list of unpacked modules, in module order.
fn module_provenance(modules: &[UnpackedModule]) -> Vec<ModuleProvenance> {
    modules
        .iter()
        .map(|module| ModuleProvenance {
            filename: module.filename.clone(),
            input: module.source_input.clone(),
            ranges: module.source_ranges.clone(),
        })
        .collect()
}

/// Provenance entry covering an entire input source.
fn whole_input_provenance(filename: &str, input: &str, source: &str) -> ModuleProvenance {
    ModuleProvenance {
        filename: filename.to_string(),
        input: input.to_string(),
        ranges: vec![(0, source.len() as u32)],
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

    let mut modules = Vec::new();

    for input in inputs {
        let result = detect_bundle_raw(&input.source, &input.filename)?.or_else(|| {
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
                let chunk_ids = webpack5::detect_chunk_ids(&input.source);
                let allow_cycle_premerge = result.allow_cycle_premerge;
                modules.extend(result.modules.into_iter().map(|module| {
                    MultiSourceModule::detected(
                        module,
                        chunk_ids.clone(),
                        input.filename.clone(),
                        allow_cycle_premerge,
                    )
                }));
            }
            None => modules.push(MultiSourceModule::fallback(UnpackedModule {
                id: input.filename.clone(),
                is_entry: false,
                code: input.source.to_string(),
                filename: filename_for_fallback_input(&input.filename),
                source_ranges: vec![(0, input.source.len() as u32)],
                source_input: input.filename.clone(),
            })),
        }
    }

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    emit_raw_modules_with_numeric_rewrites(modules, numeric_rewrite_plan)
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
    allow_cycle_premerge: bool,
    chunk_ids: HashSet<usize>,
    input_filename: String,
    input_group: String,
}

impl MultiSourceModule {
    fn detected(
        mut module: UnpackedModule,
        chunk_ids: HashSet<usize>,
        input_filename: String,
        allow_cycle_premerge: bool,
    ) -> Self {
        let input_group = input_group_for_filename(&input_filename);
        // Unpackers don't know which physical input they ran on; attribute
        // provenance ranges to it here.
        module.source_input = input_filename.clone();
        Self {
            module,
            allow_cross_chunk_rewrite: true,
            allow_cycle_premerge,
            chunk_ids,
            input_filename,
            input_group,
        }
    }

    fn fallback(module: UnpackedModule) -> Self {
        Self {
            module,
            allow_cross_chunk_rewrite: false,
            allow_cycle_premerge: false,
            chunk_ids: HashSet::new(),
            input_filename: String::new(),
            input_group: String::new(),
        }
    }
}

struct PreparedUnpackModule {
    module: UnpackedModule,
    numeric_rewrite: Option<NumericRewriteModuleContext>,
    allow_cycle_premerge: bool,
}

impl PreparedUnpackModule {
    fn plain(module: UnpackedModule) -> Self {
        Self {
            module,
            numeric_rewrite: None,
            allow_cycle_premerge: true,
        }
    }

    fn with_cycle_premerge(module: UnpackedModule, allow_cycle_premerge: bool) -> Self {
        Self {
            module,
            numeric_rewrite: None,
            allow_cycle_premerge,
        }
    }
}

struct NumericRewriteModuleContext {
    input_group: String,
}

struct Phase1PreparedModule {
    globals: Globals,
    module: Module,
    unresolved_mark: Mark,
}

struct Phase1Module {
    filename: String,
    facts: crate::facts::ModuleFacts,
    prepared: Option<Phase1PreparedModule>,
    warning: Option<UnpackWarning>,
}

#[derive(Default)]
struct NumericRewritePlan {
    plain_id_to_filename: HashMap<usize, String>,
    chunk_id_to_filename: HashMap<(String, usize, usize), String>,
}

impl NumericRewritePlan {
    fn is_empty(&self) -> bool {
        self.plain_id_to_filename.is_empty() && self.chunk_id_to_filename.is_empty()
    }
}

fn prepare_multi_source_modules(
    mut modules: Vec<MultiSourceModule>,
) -> (Vec<PreparedUnpackModule>, NumericRewritePlan) {
    assign_unique_module_filenames(&mut modules);
    let numeric_rewrite_plan = NumericRewritePlan {
        plain_id_to_filename: unique_numeric_module_id_map(&modules),
        chunk_id_to_filename: unique_numeric_chunk_module_id_map(&modules),
    };
    let has_rewrites = !numeric_rewrite_plan.is_empty();

    let modules = modules
        .into_iter()
        .map(|module| {
            let numeric_rewrite = if has_rewrites && module.allow_cross_chunk_rewrite {
                Some(NumericRewriteModuleContext {
                    input_group: module.input_group,
                })
            } else {
                None
            };
            PreparedUnpackModule {
                module: module.module,
                numeric_rewrite,
                allow_cycle_premerge: module.allow_cycle_premerge,
            }
        })
        .collect();

    (modules, numeric_rewrite_plan)
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

fn unique_numeric_module_id_map(modules: &[MultiSourceModule]) -> HashMap<usize, String> {
    let mut counts: HashMap<usize, (usize, String)> = HashMap::new();
    for module in modules {
        if !module.allow_cross_chunk_rewrite {
            continue;
        }
        let Ok(id) = module.module.id.parse::<usize>() else {
            continue;
        };
        let entry = counts
            .entry(id)
            .or_insert((0, module.module.filename.clone()));
        entry.0 += 1;
        entry.1 = module.module.filename.clone();
    }

    counts
        .into_iter()
        .filter_map(|(key, (count, filename))| (count == 1).then_some((key, filename)))
        .collect()
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

fn apply_numeric_rewrites(
    module: &mut Module,
    unresolved_mark: Mark,
    context: Option<&NumericRewriteModuleContext>,
    plan: &NumericRewritePlan,
) {
    let Some(context) = context else {
        return;
    };
    if plan.is_empty() {
        return;
    }

    module.visit_mut_with(&mut WebpackNumericReferenceRewriter {
        input_group: &context.input_group,
        unresolved_mark,
        plain_id_to_filename: &plan.plain_id_to_filename,
        chunk_id_to_filename: &plan.chunk_id_to_filename,
    });
}

fn emit_raw_modules_with_numeric_rewrites(
    modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
) -> Result<UnpackOutput> {
    let provenance: Vec<ModuleProvenance> = modules
        .iter()
        .map(|prepared| ModuleProvenance {
            filename: prepared.module.filename.clone(),
            input: prepared.module.source_input.clone(),
            ranges: prepared.module.source_ranges.clone(),
        })
        .collect();

    if numeric_rewrite_plan.is_empty() {
        return Ok(UnpackOutput {
            modules: modules
                .into_iter()
                .map(|module| (module.module.filename, module.module.code))
                .collect(),
            provenance,
            warnings: Vec::new(),
        });
    }

    let triples = modules
        .into_par_iter()
        .map(|unpacked| {
            match GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let mut module =
                    parse_js(&unpacked.module.code, &unpacked.module.filename, cm.clone())?;
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );
                module.visit_mut_with(&mut fixer(None));
                print_js(&module, cm)
            }) {
                Ok(code) => (unpacked.module.filename, code, None),
                Err(e) => {
                    let warning = UnpackWarning::new(
                        unpacked.module.filename.clone(),
                        UnpackWarningKind::RawNormalizationFailed,
                        format!("raw numeric rewrite failed, preserving unparsed code: {e}"),
                    );
                    (
                        unpacked.module.filename,
                        unpacked.module.code,
                        Some(warning),
                    )
                }
            }
        })
        .collect::<Vec<_>>();

    let mut modules = Vec::new();
    let mut warnings = Vec::new();
    for (filename, code, warning) in triples {
        modules.push((filename, code));
        if let Some(warning) = warning {
            warnings.push(warning);
        }
    }

    Ok(UnpackOutput {
        modules,
        provenance,
        warnings,
    })
}

struct WebpackNumericReferenceRewriter<'a> {
    input_group: &'a str,
    unresolved_mark: Mark,
    plain_id_to_filename: &'a HashMap<usize, String>,
    chunk_id_to_filename: &'a HashMap<(String, usize, usize), String>,
}

impl VisitMut for WebpackNumericReferenceRewriter<'_> {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        self.rewrite_async_chunk_t_bind(call);
        call.visit_mut_children_with(self);
        self.rewrite_plain_require(call);
    }
}

impl WebpackNumericReferenceRewriter<'_> {
    fn rewrite_plain_require(&self, call: &mut CallExpr) {
        let Callee::Expr(callee_expr) = &call.callee else {
            return;
        };
        let Expr::Ident(callee) = strip_parens(callee_expr) else {
            return;
        };
        if callee.sym.as_ref() != "require" || callee.ctxt.outer() != self.unresolved_mark {
            return;
        }

        let Some(module_id) = numeric_single_arg_id(call) else {
            return;
        };
        let Some(filename) = self.plain_id_to_filename.get(&module_id) else {
            return;
        };
        rewrite_numeric_arg_to_filename(&mut call.args[0], filename);
    }

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
            self.chunk_id_to_filename
                .get(&(self.input_group.to_string(), chunk_id, module_id))
        else {
            return;
        };
        rewrite_numeric_arg_to_filename(arg, filename);
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

fn numeric_single_arg_id(call: &CallExpr) -> Option<usize> {
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    numeric_arg_id(&call.args[0].expr)
}

fn rewrite_numeric_arg_to_filename(arg: &mut ExprOrSpread, filename: &str) {
    let path = format!("./{filename}");
    *arg.expr = Expr::Lit(Lit::Str(Str {
        span: DUMMY_SP,
        value: path.into(),
        raw: None,
    }));
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

fn detect_bundle_raw(source: &str, filename: &str) -> Result<Option<UnpackResult>> {
    match crate::unpacker::try_unpack_bundle_raw(source) {
        Ok(result) => Ok(result),
        Err(bundle_parse_error) => {
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
        recover_late_esm_from_factory_iifes(
            &mut module,
            unresolved_mark,
            RewriteLevel::Standard,
            LateEsmRecoveryOptions {
                smart_rename: false,
                export_rename: false,
            },
        );
        module.visit_mut_with(&mut fixer(None));
        print_js(&module, cm)
    })
}

fn recover_late_esm_from_factory_iifes(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    options: LateEsmRecoveryOptions,
) {
    module.visit_mut_with(&mut ArrowFunction);
    module.visit_mut_with(&mut ArrowReturn);
    module.visit_mut_with(&mut UnIife::new(level));
    apply_rules(
        module,
        unresolved_mark,
        RulePipelineOptions::between("UnCurlyBraces", "UnEsm").with_rewrite_level(level),
    );
    if options.smart_rename {
        module.visit_mut_with(&mut SmartRename::new(unresolved_mark));
    }
    if options.export_rename {
        module.visit_mut_with(&mut UnExportRename);
    }
    module.visit_mut_with(&mut ArrowReturn);
}

#[derive(Clone, Copy)]
struct LateEsmRecoveryOptions {
    smart_rename: bool,
    export_rename: bool,
}

impl Default for LateEsmRecoveryOptions {
    fn default() -> Self {
        Self {
            smart_rename: true,
            export_rename: true,
        }
    }
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
#[cfg(test)]
fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<UnpackOutput> {
    let modules = modules
        .into_iter()
        .map(PreparedUnpackModule::plain)
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

fn unpack_unpack_result(result: UnpackResult, options: DecompileOptions) -> Result<UnpackOutput> {
    let allow_cycle_premerge = result.allow_cycle_premerge;
    let modules = result
        .modules
        .into_iter()
        .map(|module| PreparedUnpackModule::with_cycle_premerge(module, allow_cycle_premerge))
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

fn unpack_multi_module_with_plan(
    modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
    options: DecompileOptions,
) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack_multi_module", count = modules.len());
    let _enter = span.enter();
    let report_import_cycle_warnings = modules.iter().all(|module| module.allow_cycle_premerge);
    let (modules, cycle_warnings) =
        if numeric_rewrite_plan.is_empty() && should_premerge_import_cycles(&modules) {
            let (modules, warnings) = merge_import_cycles(
                modules
                    .into_iter()
                    .map(|prepared| prepared.module)
                    .collect(),
            );
            (
                modules
                    .into_iter()
                    .map(PreparedUnpackModule::plain)
                    .collect(),
                warnings,
            )
        } else {
            // Numeric rewrite context is per original input group. A merged cycle
            // could contain members from different groups, but the later AST
            // pipeline accepts only one context per output module. Keep those
            // modules split so numeric require ids are rewritten in their original
            // context and source strings stay untouched until the normal pipeline.
            (modules, Vec::new())
        };

    // Capture provenance now: filenames are final after multi-source dedup
    // and cycle premerge, and the phase pipeline below only rewrites code.
    let provenance: Vec<ModuleProvenance> = modules
        .iter()
        .map(|prepared| ModuleProvenance {
            filename: prepared.module.filename.clone(),
            input: prepared.module.source_input.clone(),
            ranges: prepared.module.source_ranges.clone(),
        })
        .collect();

    // Parse the sourcemap once before the loop.
    let parsed_sourcemap = options
        .sourcemap
        .as_deref()
        .map(parse_sourcemap)
        .transpose()?;
    let can_reuse_phase1_ast = parsed_sourcemap.is_none();

    // Phase 1: collect facts. Run the through-UnEsm normalization range on each
    // module and extract import/export facts. For normal unpacking, keep that
    // normalized AST so Phase 2 can resume after the facts barrier. Source-map
    // mode still reparses in Phase 2 because sourcemap renaming depends on the
    // original parser SourceMap.
    let collect_facts = |unpacked: &PreparedUnpackModule| -> Phase1Module {
        let globals = Globals::new();
        let (facts, prepared_parts, warning) = GLOBALS.set(&globals, || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module =
                match parse_js(&unpacked.module.code, &unpacked.module.filename, cm.clone()) {
                    Ok(module) => module,
                    Err(e) => {
                        return (
                            crate::facts::ModuleFacts::default(),
                            None,
                            Some(UnpackWarning::new(
                                unpacked.module.filename.clone(),
                                UnpackWarningKind::FactCollectionParseFailed,
                                format!(
                                    "parse failed during fact collection, using empty facts: {e}"
                                ),
                            )),
                        );
                    }
                };
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                unpacked.numeric_rewrite.as_ref(),
                &numeric_rewrite_plan,
            );
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::until("UnEsm"),
            );
            let mut facts_module = module.clone();
            recover_late_esm_from_factory_iifes(
                &mut facts_module,
                unresolved_mark,
                RewriteLevel::Standard,
                LateEsmRecoveryOptions::default(),
            );
            let facts = collect_module_facts(&facts_module);
            let prepared = can_reuse_phase1_ast.then_some((module, unresolved_mark));
            (facts, prepared, None)
        });
        let prepared = prepared_parts.map(|(module, unresolved_mark)| Phase1PreparedModule {
            globals,
            module,
            unresolved_mark,
        });
        Phase1Module {
            filename: unpacked.module.filename.clone(),
            facts,
            prepared,
            warning,
        }
    };

    let phase1: Vec<_> = {
        let span = tracing::info_span!("phase1_collect_facts");
        let _enter = span.enter();
        modules.par_iter().map(collect_facts).collect()
    };

    let mut module_facts = ModuleFactsMap::new();
    let mut prepared_modules = Vec::with_capacity(phase1.len());
    let mut warnings = Vec::new();
    if options.diagnostics {
        warnings.extend(cycle_warnings);
    }
    for phase1_module in phase1 {
        module_facts.insert(&phase1_module.filename, phase1_module.facts);
        prepared_modules.push(phase1_module.prepared);
        if let Some(w) = phase1_module.warning {
            warnings.push(w);
        }
    }

    // Phase 2: output pipeline with late pass. Each module is parsed from
    // the original source only when Phase 1 failed to prepare an AST; otherwise
    // it continues from the Phase 1 normalized AST after the facts barrier.
    let facts_ref = &module_facts;
    let sm_ref = &parsed_sourcemap;
    let phase2_inputs: Vec<_> = modules.into_iter().zip(prepared_modules).collect();

    let decompile_module = |(unpacked, prepared): (
        PreparedUnpackModule,
        Option<Phase1PreparedModule>,
    )|
     -> (String, String, Vec<UnpackWarning>) {
        let run_phase2_tail = |mut module: Module,
                               cm: Lrc<SourceMap>,
                               unresolved_mark: Mark,
                               input_parse_warnings: Vec<UnpackWarning>|
         -> Result<(String, Vec<UnpackWarning>)> {
            // Late pass at the barrier
            run_reexport_consolidation(&mut module, facts_ref);
            run_namespace_decomposition(&mut module, facts_ref);
            // Late helper-through-UnReturn range.
            apply_rules(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::between("UnObjectSpread2", "UnReturn")
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
            recover_late_esm_from_factory_iifes(
                &mut module,
                unresolved_mark,
                options.level,
                LateEsmRecoveryOptions::default(),
            );
            module.visit_mut_with(&mut UnOptionalChaining::new(unresolved_mark, options.level));
            module.visit_mut_with(&mut UnConditionalsAssignmentOnly);
            module.visit_mut_with(&mut UnConditionals);
            prune_stale_local_named_exports(&mut module);
            dedup_duplicate_exports(&mut module);

            // Source-map-enhanced passes
            if let Some(sm) = sm_ref {
                module.visit_mut_with(&mut ImportDedup);
                apply_sourcemap_renames(&mut module, sm, &cm, unresolved_mark);
                module.visit_mut_with(&mut UnImportRename::new(unresolved_mark));
            }

            let mut diag_warnings = if options.diagnostics {
                let mut warnings = input_parse_warnings;
                warnings.extend(collect_tdz_warnings(&module, &unpacked.module.filename));
                warnings.extend(collect_duplicate_declaration_warnings(
                    &module,
                    &unpacked.module.filename,
                ));
                warnings
            } else {
                Vec::new()
            };

            module.visit_mut_with(&mut fixer(None));
            let code = print_js(&module, cm)?;

            if options.diagnostics {
                diag_warnings.extend(verify_output_parses(&code, &unpacked.module.filename));
            }

            Ok((code, diag_warnings))
        };

        let result = if let Some(prepared) = prepared {
            let Phase1PreparedModule {
                globals,
                module,
                unresolved_mark,
            } = prepared;
            GLOBALS.set(&globals, || {
                let cm: Lrc<SourceMap> = Default::default();
                run_phase2_tail(module, cm, unresolved_mark, Vec::new())
            })
        } else {
            GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let parsed = parse_js_with_recovery(
                    &unpacked.module.code,
                    &unpacked.module.filename,
                    cm.clone(),
                )?;
                let mut module = parsed.module;
                let unresolved_mark = Mark::new();
                let top_level_mark = Mark::new();
                module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );

                // Through-UnEsm range.
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
                );

                let input_parse_warnings = if options.diagnostics {
                    collect_input_parse_warnings(&parsed.recoverable_errors)
                } else {
                    Vec::new()
                };
                run_phase2_tail(module, cm, unresolved_mark, input_parse_warnings)
            })
        };

        match result {
            Ok((code, diag_warnings)) => (unpacked.module.filename, code, diag_warnings),
            Err(e) => (
                unpacked.module.filename.clone(),
                unpacked.module.code,
                vec![UnpackWarning::new(
                    unpacked.module.filename,
                    UnpackWarningKind::DecompileFailed,
                    format!("decompile failed, preserving raw code: {e}"),
                )],
            ),
        }
    };

    let triples: Vec<_> = {
        let span = tracing::info_span!("phase2_decompile_modules");
        let _enter = span.enter();
        phase2_inputs
            .into_par_iter()
            .map(decompile_module)
            .collect()
    };

    let mut modules = Vec::with_capacity(triples.len());
    for (filename, code, module_warnings) in triples {
        modules.push((filename, code));
        warnings.extend(module_warnings);
    }
    if options.diagnostics && report_import_cycle_warnings {
        warnings.extend(collect_import_cycle_warnings(&modules));
    }

    Ok(UnpackOutput {
        modules,
        provenance,
        warnings,
    })
}

fn should_merge_raw_import_cycles(_modules: &[UnpackedModule]) -> bool {
    // Keep the raw merge hook available, but disabled for now. ESM cycles are
    // often valid, and the previous repair could undo recovered module
    // boundaries before users had a chance to inspect raw output.
    false
}

fn should_premerge_import_cycles(_modules: &[PreparedUnpackModule]) -> bool {
    // Keep the pre-merge hook available for a future static validator, but do
    // not merge only because a local import SCC exists. Native ESM cycles are
    // often valid, while concatenating SCCs reduces split fidelity and can hide
    // import-synthesis bugs. Remaining cycles are reported by diagnostics for
    // non-scope-hoisted outputs.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpacker::UnpackedModule;

    #[test]
    fn scan_local_import_dependencies_reads_static_imports() {
        let module_names = ["a.js".to_string(), "nested/b.js".to_string()]
            .into_iter()
            .collect();
        let deps = scan_local_import_dependencies(
            "nested/current.js",
            r#"
import { a } from "../a.js";
import {
  b
} from "./b.js";
import fs from "fs";
const value = import("./dynamic.js");
"#,
            &module_names,
        )
        .expect("static imports should scan without parsing");

        assert_eq!(deps, vec!["a.js".to_string(), "nested/b.js".to_string()]);
    }

    #[test]
    fn scan_local_import_dependencies_ignores_import_like_body_code() {
        let module_names = ["dynamic.js".to_string()].into_iter().collect();
        let deps = scan_local_import_dependencies(
            "entry.js",
            r#"
const value = "import './dynamic.js'";
import("./dynamic.js");
"#,
            &module_names,
        )
        .expect("non-import prefix should still be a valid fast scan");

        assert!(deps.is_empty());
    }

    #[test]
    fn scan_local_import_dependencies_ignores_nested_import_like_lines() {
        let module_names = ["nested.js".to_string()].into_iter().collect();
        let deps = scan_local_import_dependencies(
            "entry.js",
            r#"
function load() {
  import { nested } from "./nested.js";
}
"#,
            &module_names,
        )
        .expect("nested import-like code should still scan without parsing");

        assert!(deps.is_empty());
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
            ..Default::default()
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
    fn detector_raw_large_scope_split_skips_runnable_cleanup_merge() {
        let mut source = String::from(
            r#"
var defProp = Object.defineProperty;
var __export = (target, all) => {
    for (var name in all)
        defProp(target, name, { get: all[name], enumerable: true });
};
var ns_a = {};
__export(ns_a, { a: () => a });
function a() { return b(); }
var ns_b = {};
__export(ns_b, { b: () => b });
function b() { return a(); }
"#,
        );
        for index in 0..1000 {
            source.push_str(&format!(
                "var ns_{index} = {{}};\n__export(ns_{index}, {{ v{index}: () => v{index} }});\nvar v{index} = {index};\n"
            ));
        }
        source.push_str("export { ns_a, ns_b };\n");

        let output = unpack_raw(&source, &DecompileOptions::default())
            .expect("large detector raw split should unpack");
        let filenames: HashSet<_> = output
            .modules
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();

        assert!(
            filenames.contains("ns_a.js") && filenames.contains("ns_b.js"),
            "detector raw output should preserve split cycle members instead of running merge cleanup"
        );
        assert!(
            output.modules.len() > 1000,
            "fixture should exercise large synthetic raw output, got {} modules",
            output.modules.len()
        );
    }

    #[test]
    fn import_cycle_premerge_is_currently_disabled() {
        let modules: Vec<PreparedUnpackModule> = (0..1025)
            .map(|index| {
                PreparedUnpackModule::plain(UnpackedModule {
                    id: format!("m{index}"),
                    is_entry: index == 0,
                    code: format!("export const m{index} = {index};"),
                    filename: if index == 0 {
                        "entry.js".to_string()
                    } else {
                        format!("m{index}.js")
                    },
                    ..Default::default()
                })
            })
            .collect();

        assert!(
            !should_premerge_import_cycles(&modules),
            "huge detector/split outputs should not pay for pre-merge repair"
        );
        assert!(
            !should_premerge_import_cycles(&modules[..1024]),
            "cycle pre-merge is currently disabled even for normal-sized outputs"
        );

        let mut scope_split_modules: Vec<_> = modules[..3]
            .iter()
            .map(|module| {
                PreparedUnpackModule::with_cycle_premerge(
                    UnpackedModule {
                        id: module.module.id.clone(),
                        is_entry: module.module.is_entry,
                        code: module.module.code.clone(),
                        filename: module.module.filename.clone(),
                        ..Default::default()
                    },
                    false,
                )
            })
            .collect();
        assert!(
            !should_premerge_import_cycles(&scope_split_modules),
            "scope-hoisted esbuild/Bun splits opt out even when small"
        );
        scope_split_modules[0].allow_cycle_premerge = true;
        assert!(
            !should_premerge_import_cycles(&scope_split_modules),
            "all modules in the output must opt in before premerge runs"
        );

        let raw_modules: Vec<UnpackedModule> = modules
            .iter()
            .take(2)
            .map(|module| UnpackedModule {
                id: module.module.id.clone(),
                is_entry: module.module.is_entry,
                code: module.module.code.clone(),
                filename: module.module.filename.clone(),
                ..Default::default()
            })
            .collect();
        assert!(
            !should_merge_raw_import_cycles(&raw_modules),
            "raw cycle merging is also kept disabled behind its gate"
        );
    }

    #[test]
    fn scope_split_cycles_do_not_emit_diagnostic_warnings() {
        let modules = vec![
            PreparedUnpackModule::with_cycle_premerge(
                UnpackedModule {
                    id: "a".to_string(),
                    is_entry: true,
                    code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                    filename: "entry.js".to_string(),
                    ..Default::default()
                },
                false,
            ),
            PreparedUnpackModule::with_cycle_premerge(
                UnpackedModule {
                    id: "b".to_string(),
                    is_entry: false,
                    code: r#"import { a } from "./entry.js"; export const b = a + 1;"#.to_string(),
                    filename: "b.js".to_string(),
                    ..Default::default()
                },
                false,
            ),
        ];

        let output = unpack_multi_module_with_plan(
            modules,
            NumericRewritePlan::default(),
            DecompileOptions {
                diagnostics: true,
                ..Default::default()
            },
        )
        .expect("scope split cycle should decompile");

        assert!(
            output.warnings.is_empty(),
            "native ESM cycles from scope splits should not produce stderr warnings: {:?}",
            output.warnings
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
    fn multi_module_split_sequence_uses_member_name_for_assignment_temp() {
        let modules = vec![UnpackedModule {
            id: "1".to_string(),
            is_entry: false,
            code: r#"var i, a, o;
module.exports = (a = (i = require("./module-2.js")).lib, o = a.WordArray, i.SHA1);
"#
            .to_string(),
            filename: "module-1.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("fixture should decompile");
        let code = &output.modules[0].1;
        assert!(
            code.contains("const lib ="),
            "expected temp binding to use member name:\n{code}"
        );
        assert!(
            !code.contains("const _a ="),
            "should not synthesize the fallback assignment name:\n{code}"
        );
    }

    #[test]
    fn multi_module_preserves_lowered_interop_binding_read_until_import_recovery() {
        let modules = vec![UnpackedModule {
            id: "1".to_string(),
            is_entry: false,
            code: r#""use strict";
Object.defineProperty(exports, "__esModule", {
    value: true
});
var a = require("./module-2.js"), o = (r(a), r(require("./module-3.js")));
function r(e) {
    return e && e.__esModule ? e : {
        default: e
    };
}
class l extends a.Component {}
exports.default = o.default(l);
"#
            .to_string(),
            filename: "module-1.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("fixture should decompile");
        let code = &output.modules[0].1;
        assert!(
            code.contains("import a from \"./module-2.js\";"),
            "expected require binding to become an import:\n{code}"
        );
        assert!(
            code.contains("import o from \"./module-3.js\";"),
            "expected interop require to become an import:\n{code}"
        );
        assert!(
            code.contains("a;\nclass l extends a.Component"),
            "expected lowered interop binding read to survive until import recovery:\n{code}"
        );
    }

    #[test]
    fn numeric_rewrite_plan_applies_to_existing_ast_without_source_stabilization() {
        let modules = vec![
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "20".to_string(),
                    is_entry: false,
                    code: "const other = require(999);".to_string(),
                    filename: "module-20.js".to_string(),
                    ..Default::default()
                },
                HashSet::new(),
                "entry.js".to_string(),
                true,
            ),
            MultiSourceModule::detected(
                UnpackedModule {
                    id: "999".to_string(),
                    is_entry: false,
                    code: "export default 1;".to_string(),
                    filename: "module-999.js".to_string(),
                    ..Default::default()
                },
                HashSet::new(),
                "chunk.js".to_string(),
                true,
            ),
        ];

        let (prepared, plan) = prepare_multi_source_modules(modules);
        assert!(
            prepared[0].module.code.contains("require(999)"),
            "prepare should keep source strings untouched"
        );

        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                &prepared[0].module.code,
                &prepared[0].module.filename,
                cm.clone(),
            )
            .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
            apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                prepared[0].numeric_rewrite.as_ref(),
                &plan,
            );
            module.visit_mut_with(&mut fixer(None));
            print_js(&module, cm).expect("fixture should print")
        });

        assert!(
            output.contains(r#"require("./module-999.js")"#),
            "rewrite plan should apply to the already-parsed AST:\n{output}"
        );
    }

    #[test]
    fn unpack_prunes_exports_for_inlined_local_aliases() {
        let modules = vec![UnpackedModule {
            id: "helper".to_string(),
            is_entry: false,
            code: r#"
var create = Object.create;
function wrap(value) {
    return create(value);
}
export { create, wrap };
"#
            .to_string(),
            filename: "helper.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                level: RewriteLevel::Standard,
                ..Default::default()
            },
        )
        .expect("module should decompile");
        let code = &output.modules[0].1;

        assert!(
            !code.contains("create }") && !code.contains("create,"),
            "inlined alias should not remain exported:\n{code}"
        );
        assert!(
            code.contains("wrap"),
            "live export should be preserved:\n{code}"
        );
    }

    #[test]
    fn normal_unpack_phase_preserves_helper_declaration_order() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"
setup();
const { defineProperty } = Object;
var helper = (target) => defineProperty({}, "x", { value: target });
function setup() {
    return helper;
}
export { helper };
"#
            .to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                level: RewriteLevel::Minimal,
                dead_code_elimination: false,
                ..Default::default()
            },
        )
        .expect("module should decompile");
        let code = &output.modules[0].1;
        let setup_call = code.find("setup()").expect("setup call should remain");
        let define_property = code
            .find("defineProperty } = Object")
            .expect("Object destructuring helper should remain");
        let helper = code.find("helper =").expect("helper binding should remain");

        assert!(
            setup_call < define_property && define_property < helper,
            "normal unpack should preserve declaration order; raw runnable cleanup owns helper hoisting:\n{code}"
        );
    }

    #[test]
    fn import_cycle_warnings_report_local_sccs() {
        let modules = vec![
            (
                "a.js".to_string(),
                r#"import { b } from "./b.js"; export const a = b;"#.to_string(),
            ),
            (
                "b.js".to_string(),
                r#"import { a } from "./a.js"; export const b = a;"#.to_string(),
            ),
            (
                "c.js".to_string(),
                r#"import { a } from "./a.js"; export const c = a;"#.to_string(),
            ),
        ];

        let warnings = collect_import_cycle_warnings(&modules);

        assert_eq!(warnings.len(), 1, "should report one SCC: {warnings:?}");
        assert_eq!(warnings[0].kind, UnpackWarningKind::ImportCycle);
        assert!(warnings[0].message.contains("2 modules"));
        assert!(warnings[0].message.contains("a.js"));
        assert!(warnings[0].message.contains("b.js"));
    }

    #[test]
    fn merge_import_cycles_drops_internal_imports_and_retargets_consumers() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "c".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const c = b;"#.to_string(),
                filename: "c.js".to_string(),
                ..Default::default()
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert!(
            warnings.is_empty(),
            "successful cycle repair should not surface as stderr warnings: {warnings:?}"
        );
        assert_eq!(merged.len(), 2);
        let a = merged
            .iter()
            .find(|module| module.filename == "a.js")
            .expect("cycle should merge into first module");
        assert!(
            !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
            "merged cycle should drop internal imports and retain member code:\n{}",
            a.code
        );
        let c = merged
            .iter()
            .find(|module| module.filename == "c.js")
            .expect("consumer should remain separate");
        assert!(
            c.code.contains("from \"./a.js\""),
            "consumer should retarget imports to merged representative:\n{}",
            c.code
        );
    }

    #[test]
    fn merge_import_cycles_does_not_reprint_unrelated_modules() {
        let untouched_code = "const untouched = 1   ;";
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; export const b = a + 1;"#.to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "d".to_string(),
                is_entry: false,
                code: untouched_code.to_string(),
                filename: "d.js".to_string(),
                ..Default::default()
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert!(
            warnings.is_empty(),
            "successful cycle repair should not surface as stderr warnings: {warnings:?}"
        );
        let unrelated = merged
            .iter()
            .find(|module| module.filename == "d.js")
            .expect("unrelated module should remain");
        assert_eq!(unrelated.code, untouched_code);
    }

    #[test]
    fn merge_import_cycles_dedups_external_imports_before_safety_check() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { b } from "./b.js"; export const a = b + shared;"#
                    .to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { shared } from "./x.js"; import { a } from "./a.js"; export const b = a + shared;"#
                    .to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
        assert!(
            warnings.is_empty(),
            "duplicate external imports should not block a safe merge or emit stderr warnings: {:?}",
            warnings
        );
        let a = &merged[0];
        assert_eq!(a.filename, "a.js");
        assert_eq!(
            a.code.matches("from \"./x.js\"").count(),
            1,
            "merged cycle should deduplicate external imports:\n{}",
            a.code
        );
        assert!(
            !a.code.contains("from \"./b.js\"") && a.code.contains("export const b"),
            "merged cycle should drop internal imports and retain member code:\n{}",
            a.code
        );
    }

    #[test]
    fn merge_import_cycles_dedups_redundant_named_exports() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; export function f() { return b; }"#
                    .to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { f } from "./a.js"; export const b = 1; export { f };"#
                    .to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 1, "warnings: {warnings:?}");
        let a = &merged[0];
        assert!(
            a.code.contains("export function f"),
            "merged cycle should keep the declaration export:\n{}",
            a.code
        );
        assert!(
            !a.code.contains("export { f"),
            "merged cycle should remove the redundant named export:\n{}",
            a.code
        );
    }

    #[test]
    fn hoist_late_runtime_helpers_moves_helper_defs_before_side_effects() {
        let input = r#"
setup();
result = helper(value);
const { defineProperty } = Object;
var helper = (target) => defineProperty({}, "x", { value: target });
let cache;
function setup() {}
consumer = wrap(ns);
export var ns = {};
Object.defineProperty(ns, "value", { enumerable: true, get: () => value });
export { helper, cache };
"#;

        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(input, "fixture.js", cm.clone()).expect("input parses");
            hoist_late_runtime_helpers(&mut module);
            print_js(&module, cm).expect("output prints")
        });

        let define_property = output
            .find("const { defineProperty")
            .expect("object destructuring helper should remain");
        let helper = output
            .find("var helper")
            .expect("helper declaration should remain");
        let cache = output
            .find("let cache")
            .expect("state declaration should remain");
        let call = output.find("result = helper").expect("call should remain");
        let namespace = output
            .find("export var ns")
            .expect("namespace export should remain");
        let namespace_getter = output
            .find("Object.defineProperty(ns")
            .expect("namespace getter should remain");
        let namespace_use = output.find("consumer = wrap").expect("use should remain");

        assert!(
            define_property < call && helper < call && cache < call,
            "late helper declarations should move before side effects:\n{output}"
        );
        assert!(
            namespace < namespace_use && namespace_getter < namespace_use,
            "late namespace export setup should move before side effects:\n{output}"
        );
    }

    #[test]
    fn merge_import_cycles_skips_duplicate_declaration_merges() {
        let modules = vec![
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code:
                    r#"import { b } from "./b.js"; const shared = 1; export const a = b + shared;"#
                        .to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code:
                    r#"import { a } from "./a.js"; const shared = 2; export const b = a + shared;"#
                        .to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
        ];

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 2, "unsafe cycles should stay split");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].message.contains("not merged")
                && warnings[0].message.contains("duplicate declarations"),
            "warning should explain why the cycle stayed split: {:?}",
            warnings
        );
        let a = merged
            .iter()
            .find(|module| module.filename == "a.js")
            .expect("a.js should remain separate");
        assert!(
            a.code.contains("from \"./b.js\""),
            "skipped cycle should preserve original imports:\n{}",
            a.code
        );
    }

    #[test]
    fn merge_import_cycles_skips_large_components() {
        let modules: Vec<UnpackedModule> = (0..33)
            .map(|index| {
                let next = (index + 1) % 33;
                UnpackedModule {
                    id: format!("m{index}"),
                    is_entry: false,
                    code: format!(
                        r#"import {{ v{next} }} from "./m{next}.js"; export const v{index} = v{next} + {index};"#
                    ),
                    filename: format!("m{index}.js"),
                    ..Default::default()
                }
            })
            .collect();

        let (merged, warnings) = merge_import_cycles(modules);

        assert_eq!(merged.len(), 33, "large cycles should stay split");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].message.contains("not merged")
                && warnings[0].message.contains("large-cycle merge limit"),
            "warning should explain why the large cycle stayed split: {:?}",
            warnings
        );
    }

    #[test]
    fn fast_cycle_preflight_allows_duplicate_var_declarations() {
        let modules = [
            UnpackedModule {
                id: "a".to_string(),
                is_entry: false,
                code: r#"import { b } from "./b.js"; var shared = 1; export const a = b + shared;"#
                    .to_string(),
                filename: "a.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "b".to_string(),
                is_entry: false,
                code: r#"import { a } from "./a.js"; var shared = 2; export const b = a + shared;"#
                    .to_string(),
                filename: "b.js".to_string(),
                ..Default::default()
            },
        ];
        let module_by_filename: HashMap<String, &UnpackedModule> = modules
            .iter()
            .map(|module| (module.filename.clone(), module))
            .collect();
        let module_names: HashSet<String> = modules
            .iter()
            .map(|module| module.filename.clone())
            .collect();
        let members = vec!["a.js".to_string(), "b.js".to_string()];
        let member_set: HashSet<String> = members.iter().cloned().collect();

        assert!(
            unsafe_merge_member_reason(&members, &module_by_filename, &module_names, &member_set)
                .is_none(),
            "generated duplicate vars should not block the large-cycle fast preflight"
        );
    }
}
