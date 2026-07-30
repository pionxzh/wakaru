use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::{apply_fixer, parse_js, print_js};
use super::types::{
    CapturedUnpackOutput, DecompileOptions, ModuleProvenance, PreparedInputId,
    PreparedUnpackOutput, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind,
};
#[cfg(test)]
use super::unpack_cycles::{collect_import_cycle_warnings, scan_local_import_dependencies};
use super::{DriverError, DriverErrorKind, DriverResult};
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, RewriteLevel, RulePipelineOptions, SmartRename, UnEsm,
    UnExportRename, UnIife,
};
use crate::unpacker::{
    scope_hoist, try_prepare_bundle, try_prepare_source, BundleFormat, DetectedBundle,
    PreparedModuleAst, PreparedSource, UnpackResult, UnpackedModule,
};

mod dead_module;
mod filename_recovery;
mod merge;
mod phases;
mod scope_split;
mod webpack_commonjs_runtime;

use merge::{
    emit_raw_modules_with_numeric_rewrites, input_group_for_filename, prepare_multi_source_modules,
    MultiSourceModule,
};
#[cfg(test)]
use phases::unpack_multi_module;
use phases::unpack_multi_module_with_plan_and_capture;
use scope_split::{maybe_split_scope_hoisted_modules, maybe_split_scope_hoisted_modules_excluding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedInputDetection {
    Bundle(BundleFormat),
    ScopeHoisted,
    Plain,
}

/// Internal scope-hoist profile used by the public façade.
///
/// This is exported through `wakaru_core::driver` only because the façade is a
/// separate lockstep crate. It is not a supported integration surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopeHoistPolicy {
    Disabled,
    #[default]
    Fallback,
    Recursive,
    Inspect,
}

impl ScopeHoistPolicy {
    fn heuristic_enabled(self) -> bool {
        self != Self::Disabled
    }

    fn recursive(self) -> bool {
        matches!(self, Self::Recursive | Self::Inspect)
    }

    fn render_mode(self) -> scope_hoist::ScopeHoistRenderMode {
        match self {
            Self::Inspect => scope_hoist::ScopeHoistRenderMode::Inspect,
            Self::Disabled | Self::Fallback | Self::Recursive => {
                scope_hoist::ScopeHoistRenderMode::Executable
            }
        }
    }
}

/// Opaque input prepared by the public façade's incremental intake path.
///
/// Structural bundle detection is complete, but the shared cross-module
/// phases are deferred until the whole logical input set is available.
pub struct PreparedUnpackInput {
    filename: String,
    source: Option<String>,
    detection: PreparedInputDetection,
    detected: Option<DetectedBundle>,
    scope_hoisted: Option<UnpackResult>,
    plain_prepared: Option<PreparedModuleAst>,
    public_path_candidate: bool,
}

impl PreparedUnpackInput {
    pub fn detection(&self) -> PreparedInputDetection {
        self.detection
    }

    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn into_plain_source(self) -> Option<(String, String)> {
        (self.detection == PreparedInputDetection::Plain).then(|| {
            (
                self.filename,
                self.source.expect("plain input retains source"),
            )
        })
    }
}

pub fn prepare_unpack_input(
    filename: String,
    source: String,
    heuristic_scope_hoist: bool,
    prepare_plain_ast: bool,
) -> DriverResult<PreparedUnpackInput> {
    prepare_unpack_input_with_policy(
        filename,
        source,
        prepare_plain_ast,
        if heuristic_scope_hoist {
            ScopeHoistPolicy::Fallback
        } else {
            ScopeHoistPolicy::Disabled
        },
    )
}

pub fn prepare_unpack_input_with_policy(
    filename: String,
    source: String,
    prepare_plain_ast: bool,
    scope_hoist_policy: ScopeHoistPolicy,
) -> DriverResult<PreparedUnpackInput> {
    let prepared = match try_prepare_source(&source, &filename, prepare_plain_ast) {
        Ok(prepared) => prepared,
        Err(bundle_parse_error) => {
            // Bundle detection deliberately uses the ES/JSX grammar. Preserve
            // filename-driven syntax (for example TypeScript) as a valid plain
            // input, but do not pretend its incompatible AST can be reused.
            let input_parse_result = GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                parse_js(&source, &filename, cm)
            });
            match input_parse_result {
                Ok(_) => {
                    return Ok(PreparedUnpackInput {
                        filename,
                        source: Some(source),
                        detection: PreparedInputDetection::Plain,
                        detected: None,
                        scope_hoisted: None,
                        plain_prepared: None,
                        public_path_candidate: false,
                    });
                }
                Err(input_parse_error) => {
                    return Err(DriverError::new(
                        DriverErrorKind::Parse,
                        anyhow!(
                            "{input_parse_error}; bundle detection also failed: {bundle_parse_error}"
                        ),
                    ));
                }
            }
        }
    };

    let mut plain_prepared = match prepared {
        PreparedSource::Bundle(detected) => {
            let format = detected.result.format;
            let public_path_candidate =
                format == BundleFormat::Esbuild && detected.input_has_esm_declarations;
            let needs_boundary_fallback = public_path_candidate
                && detected
                    .result
                    .modules
                    .iter()
                    .filter(|module| module.is_entry)
                    .count()
                    != 1;
            let fallback_prepared = needs_boundary_fallback
                .then(|| prepare_plain_ast_for_filename(&source, &filename))
                .transpose()
                .map_err(|error| DriverError::new(DriverErrorKind::Parse, error))?;
            return Ok(PreparedUnpackInput {
                filename,
                source: needs_boundary_fallback.then_some(source),
                detection: PreparedInputDetection::Bundle(format),
                detected: Some(detected),
                scope_hoisted: None,
                plain_prepared: fallback_prepared,
                public_path_candidate,
            });
        }
        PreparedSource::Plain(prepared) => prepared,
    };

    // Detection always starts with the ES/JSX grammar. If that parser only
    // produced an AST by recovering errors, prefer a clean filename-driven
    // parse (notably for TypeScript) before deciding the AST is reusable.
    if matches!(
        std::path::Path::new(&filename)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("ts" | "tsx")
    ) && plain_prepared
        .as_ref()
        .is_some_and(|prepared| !prepared.recoverable_parse_errors.is_empty())
    {
        if let Ok(prepared) = prepare_plain_ast_for_filename(&source, &filename) {
            plain_prepared = Some(prepared);
        }
    }

    if scope_hoist_policy.heuristic_enabled() {
        if let Some(result) = scope_hoist::split_scope_hoisted_with_mode(
            &source,
            scope_hoist_policy.render_mode(),
            scope_hoist::ScopeHoistSource::DirectAsset,
        )
        .filter(|result| result.modules.len() > 1)
        {
            let needs_boundary_fallback = result
                .modules
                .iter()
                .filter(|module| module.is_entry)
                .count()
                != 1;
            let fallback_prepared = needs_boundary_fallback
                .then(|| prepare_plain_ast_for_filename(&source, &filename))
                .transpose()
                .map_err(|error| DriverError::new(DriverErrorKind::Parse, error))?;
            return Ok(PreparedUnpackInput {
                filename,
                source: needs_boundary_fallback.then_some(source),
                detection: PreparedInputDetection::ScopeHoisted,
                detected: None,
                scope_hoisted: Some(result),
                plain_prepared: fallback_prepared,
                public_path_candidate: true,
            });
        }
    }

    Ok(PreparedUnpackInput {
        filename,
        source: Some(source),
        detection: PreparedInputDetection::Plain,
        detected: None,
        scope_hoisted: None,
        plain_prepared,
        public_path_candidate: false,
    })
}

fn prepare_plain_ast_for_filename(source: &str, filename: &str) -> Result<PreparedModuleAst> {
    let globals = swc_core::common::Globals::new();
    let (module, unresolved_mark) = GLOBALS.set(&globals, || {
        let cm: Lrc<SourceMap> = Default::default();
        let mut module = parse_js(source, filename, cm)?;
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        Ok::<_, anyhow::Error>((module, unresolved_mark))
    })?;
    Ok(PreparedModuleAst {
        globals,
        module,
        unresolved_mark,
        recoverable_parse_errors: Vec::new(),
    })
}

fn plan_public_paths(
    inputs: &[PreparedUnpackInput],
    raw: bool,
) -> Result<merge::PlannedPublicPaths> {
    let mut planned = merge::PlannedPublicPaths::default();
    if raw || inputs.len() < 2 {
        return Ok(planned);
    }

    let absolute_root = common_absolute_input_parent(inputs);
    let mut claimed = HashSet::new();
    // Every physical ESM identity claims its public path before generated
    // names are assigned. This includes reusable facades and plain inputs;
    // script-loaded bundle inputs have no relative-ESM identity in output.
    // Any collision is fatal because suffixing either physical input would
    // silently break author-written references from sibling inputs.
    for (index, input) in inputs.iter().enumerate() {
        let is_facade = input.public_path_candidate;
        if !is_facade && input.detection != PreparedInputDetection::Plain {
            continue;
        }
        let public_path = public_path_for_input(&input.filename, absolute_root.as_deref())?;
        let collision_key = public_path.to_lowercase();
        if !claimed.insert(collision_key) {
            return Err(anyhow!(
                "ambiguous public module path {:?}: multiple processed inputs claim the same normalized path",
                public_path
            ));
        }
        let input_id = PreparedInputId::from_index(index);
        if is_facade {
            planned.facade.insert(input_id, public_path.clone());
        }
        planned.input.insert(input_id, public_path);
    }
    Ok(planned)
}

fn common_absolute_input_parent(inputs: &[PreparedUnpackInput]) -> Option<PathBuf> {
    // Relative inputs derive their public paths from their own relative
    // structure; they must not veto the shared root of the absolute inputs
    // (collapsing those to bare basenames invites spurious collisions).
    let normalized = inputs
        .iter()
        .map(|input| merge::normalize_path_lexically(Path::new(&input.filename)))
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return None;
    }

    let mut common = normalized[0].parent()?.to_path_buf();
    for path in &normalized[1..] {
        while !path.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    Some(common)
}

fn public_path_for_input(filename: &str, absolute_root: Option<&Path>) -> Result<String> {
    let normalized = merge::normalize_path_lexically(Path::new(filename));
    let candidate = if normalized.is_absolute() {
        absolute_root
            .and_then(|root| normalized.strip_prefix(root).ok())
            .map(Path::to_path_buf)
            .or_else(|| normalized.file_name().map(PathBuf::from))
            .ok_or_else(|| anyhow!("cannot derive a public output path from {filename:?}"))?
    } else {
        // `wakaru --unpack ../pkg/bundle.js` is an ordinary invocation; the
        // traversal prefix cannot be mirrored under the output root, so keep
        // the in-bounds remainder rather than failing the whole run.
        // Lexical normalization leaves `..` components only at the front.
        normalized
            .components()
            .skip_while(|component| matches!(component, std::path::Component::ParentDir))
            .collect()
    };
    let safe = super::output::safe_relative_module_path(&candidate.to_string_lossy())?;
    Ok(safe.to_string_lossy().replace('\\', "/"))
}

fn public_boundary_fallback_module(
    input: PreparedInputId,
    public_path: String,
    source: String,
    prepared: Option<PreparedModuleAst>,
) -> MultiSourceModule {
    let source_len = source.len() as u32;
    MultiSourceModule::fallback_with_ast_from_input(
        UnpackedModule {
            id: public_path.clone(),
            is_entry: true,
            filename: public_path,
            source_ranges: vec![(0, source_len)],
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
            code: source,
        },
        prepared,
        Some(input),
    )
}

pub fn unpack_prepared_inputs(
    inputs: Vec<PreparedUnpackInput>,
    options: DecompileOptions,
    raw: bool,
    recursive_scope_hoist: bool,
) -> Result<PreparedUnpackOutput> {
    unpack_prepared_inputs_with_policy(
        inputs,
        options,
        raw,
        if recursive_scope_hoist {
            ScopeHoistPolicy::Recursive
        } else {
            ScopeHoistPolicy::Fallback
        },
    )
}

pub fn unpack_prepared_inputs_with_policy(
    inputs: Vec<PreparedUnpackInput>,
    options: DecompileOptions,
    raw: bool,
    scope_hoist_policy: ScopeHoistPolicy,
) -> Result<PreparedUnpackOutput> {
    unpack_prepared_inputs_with_policy_and_capture(inputs, options, raw, scope_hoist_policy, false)
        .map(|captured| captured.output)
}

#[doc(hidden)]
pub fn unpack_prepared_inputs_with_policy_and_capture(
    inputs: Vec<PreparedUnpackInput>,
    options: DecompileOptions,
    raw: bool,
    scope_hoist_policy: ScopeHoistPolicy,
    capture_pre_rewrite: bool,
) -> Result<CapturedUnpackOutput> {
    unpack_prepared_inputs_with_policies_and_capture(
        inputs
            .into_iter()
            .map(|input| (input, scope_hoist_policy))
            .collect(),
        options,
        raw,
        capture_pre_rewrite,
    )
}

#[doc(hidden)]
pub fn unpack_prepared_inputs_with_policies_and_capture(
    inputs: Vec<(PreparedUnpackInput, ScopeHoistPolicy)>,
    mut options: DecompileOptions,
    raw: bool,
    capture_pre_rewrite: bool,
) -> Result<CapturedUnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one prepared input is required"));
    }

    let public_paths = plan_public_paths(&inputs, raw)?;

    let mut modules = Vec::new();
    let mut detected_formats = Vec::new();
    let mut preparation_warnings = Vec::new();
    for (input_index, (input, scope_hoist_policy)) in inputs.into_iter().enumerate() {
        let input_id = PreparedInputId::from_index(input_index);
        let PreparedUnpackInput {
            filename,
            source,
            detection,
            detected,
            scope_hoisted,
            plain_prepared,
            public_path_candidate: _,
        } = input;
        match detection {
            PreparedInputDetection::Bundle(format) => {
                if !detected_formats.contains(&format) {
                    detected_formats.push(format);
                }
                let detected = detected.expect("bundle detection carries prepared result");
                if format == BundleFormat::Esbuild
                    && public_paths.facade.contains_key(&input_id)
                    && detected
                        .result
                        .modules
                        .iter()
                        .filter(|module| module.is_entry)
                        .count()
                        != 1
                {
                    modules.push(public_boundary_fallback_module(
                        input_id,
                        public_paths.facade[&input_id].clone(),
                        source.expect("unprovable esbuild boundary retains source"),
                        plain_prepared,
                    ));
                    continue;
                }
                let chunk_ids = Arc::new(detected.chunk_ids.clone());
                // Capture this detector-owned fact before optional materialization or
                // recursive scope splitting. Empty webpack factories cannot split, so
                // their stable `(id, filename)` identity survives both paths, while an
                // empty synthetic child can never acquire the factory runtime fact.
                let implicit_commonjs_default_objects =
                    if matches!(format, BundleFormat::Webpack4 | BundleFormat::Webpack5) {
                        detected
                            .result
                            .modules
                            .iter()
                            .zip(&detected.prepared)
                            .filter(|(module, prepared)| {
                                prepared
                                    .as_ref()
                                    .map(|prepared| prepared.module.body.is_empty())
                                    .unwrap_or_else(|| module.code.trim().is_empty())
                            })
                            .map(|(module, _)| (module.id.clone(), module.filename.clone()))
                            .collect::<HashSet<_>>()
                    } else {
                        HashSet::new()
                    };
                // Keep this proof on modules whose original detector identity
                // survives optional recursive splitting. Synthetic children do
                // not automatically inherit a webpack factory runtime.
                let webpack_commonjs_runtime_modules =
                    if matches!(format, BundleFormat::Webpack4 | BundleFormat::Webpack5) {
                        detected
                            .result
                            .modules
                            .iter()
                            .map(|module| (module.id.clone(), module.filename.clone()))
                            .collect::<HashSet<_>>()
                    } else {
                        HashSet::new()
                    };
                let webpack_numeric_module_ids = detected
                    .result
                    .modules
                    .iter()
                    .filter_map(|module| {
                        detected
                            .webpack_numeric_module_ids
                            .get(&module.filename)
                            .copied()
                            .map(|runtime_id| {
                                ((module.id.clone(), module.filename.clone()), runtime_id)
                            })
                    })
                    .collect::<HashMap<_, _>>();
                let webpack_legacy_module_i = detected
                    .result
                    .modules
                    .iter()
                    .filter(|module| detected.webpack_legacy_module_i.contains(&module.filename))
                    .map(|module| (module.id.clone(), module.filename.clone()))
                    .collect::<HashSet<_>>();
                let detected = if raw {
                    let result = detected.materialize()?;
                    DetectedBundle::from_result(maybe_split_scope_hoisted_modules(
                        result,
                        scope_hoist_policy.recursive(),
                        scope_hoist_policy.render_mode(),
                    ))
                } else {
                    maybe_split_detected_bundle(
                        detected,
                        scope_hoist_policy.recursive(),
                        options.emit_source_map,
                        scope_hoist_policy.render_mode(),
                    )?
                };
                let (result, prepared, module_failures) = detected.into_parts();
                let has_module_failures = !module_failures.is_empty();
                let report_import_cycle_warnings = result.report_import_cycle_warnings;
                let input_group = input_group_for_filename(&filename);
                modules.extend(
                    result
                        .modules
                        .into_iter()
                        .zip(prepared)
                        .map(|(module, ast)| {
                            let detector_failure = module_failures.get(&module.filename).copied();
                            let implicit_commonjs_default_object =
                                implicit_commonjs_default_objects
                                    .contains(&(module.id.clone(), module.filename.clone()));
                            let webpack_commonjs_runtime = webpack_commonjs_runtime_modules
                                .contains(&(module.id.clone(), module.filename.clone()));
                            let webpack_numeric_module_id = webpack_numeric_module_ids
                                .get(&(module.id.clone(), module.filename.clone()))
                                .copied();
                            let webpack_legacy_module_i = webpack_legacy_module_i
                                .contains(&(module.id.clone(), module.filename.clone()));
                            MultiSourceModule::detected_with_ast_from_input(
                                module,
                                ast,
                                chunk_ids.clone(),
                                filename.clone(),
                                Some(input_id),
                                input_group.clone(),
                                report_import_cycle_warnings,
                            )
                            .with_implicit_commonjs_default_object(implicit_commonjs_default_object)
                            .with_webpack_commonjs_runtime(webpack_commonjs_runtime)
                            .with_webpack_numeric_module_id(webpack_numeric_module_id)
                            .with_webpack_legacy_module_i(webpack_legacy_module_i)
                            // Intra-container edges were already rewritten by
                            // the detector. If any local ID is opaque, do not
                            // let a same-numbered module from another input
                            // capture the deliberately unresolved call.
                            .with_cross_chunk_rewrite(!has_module_failures)
                            .with_detector_failure(detector_failure)
                        }),
                );
            }
            PreparedInputDetection::ScopeHoisted => {
                if !detected_formats.contains(&BundleFormat::ScopeHoisted) {
                    detected_formats.push(BundleFormat::ScopeHoisted);
                }
                let result = scope_hoisted.expect("scope-hoist detection carries result");
                if public_paths.facade.contains_key(&input_id)
                    && result
                        .modules
                        .iter()
                        .filter(|module| module.is_entry)
                        .count()
                        != 1
                {
                    modules.push(public_boundary_fallback_module(
                        input_id,
                        public_paths.facade[&input_id].clone(),
                        source.expect("unprovable scope boundary retains source"),
                        plain_prepared,
                    ));
                    continue;
                }
                modules.extend(result.modules.into_iter().map(|mut module| {
                    if raw {
                        match normalize_raw_unpacked_module(&module.code, &module.filename) {
                            Ok(normalized) => module.code = normalized,
                            Err(error) => preparation_warnings.push(UnpackWarning::new(
                                module.filename.clone(),
                                UnpackWarningKind::RawNormalizationFailed,
                                format!(
                                    "raw normalization failed, preserving unparsed code: {error}"
                                ),
                            )),
                        }
                    }
                    MultiSourceModule::fallback_with_ast_from_input(module, None, Some(input_id))
                }));
            }
            PreparedInputDetection::Plain => {
                let source = source.expect("plain input retains source");
                let source_len = source.len() as u32;
                modules.push(MultiSourceModule::fallback_with_ast_from_input(
                    UnpackedModule {
                        id: filename.clone(),
                        is_entry: false,
                        filename: public_paths
                            .input
                            .get(&input_id)
                            .cloned()
                            .unwrap_or_else(|| filename_for_fallback_input(&filename)),
                        source_ranges: vec![(0, source_len)],
                        inspection_context_ranges: Vec::new(),
                        source_input: String::new(),
                        generated_source_map: Vec::new(),
                        code: source,
                    },
                    (!raw).then_some(plain_prepared).flatten(),
                    Some(input_id),
                ));
            }
        }
    }

    if !raw && detected_formats == [BundleFormat::ScopeHoisted] && modules.len() > 1 {
        options.dce_mode = super::types::DceMode::Off;
    }
    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules, &public_paths);
    let mut captured = if raw {
        CapturedUnpackOutput {
            output: emit_raw_modules_with_numeric_rewrites(modules, numeric_rewrite_plan)?,
            pre_rewrite_modules: Vec::new(),
        }
    } else {
        unpack_multi_module_with_plan_and_capture(
            modules,
            numeric_rewrite_plan,
            options.clone(),
            capture_pre_rewrite,
        )?
    };
    captured.output.warnings.splice(0..0, preparation_warnings);
    captured.output.detected_formats = detected_formats;
    Ok(captured)
}

pub fn unpack(source: &str, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack");
    let _enter = span.enter();
    unpack_legacy_inputs(
        vec![UnpackInput {
            filename: options.filename.clone(),
            source: source.to_string(),
        }],
        options,
        false,
    )
}

pub fn unpack_files(inputs: Vec<UnpackInput>, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack_files", count = inputs.len());
    let _enter = span.enter();
    unpack_legacy_inputs(inputs, options, false)
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns raw module output after detector-specific extraction and
/// bundler-coupled normalization. Cross-module analysis and the normal
/// decompile rule pipeline are skipped.
pub fn unpack_raw(source: &str, options: &DecompileOptions) -> Result<UnpackOutput> {
    unpack_legacy_inputs(
        vec![UnpackInput {
            filename: options.filename.clone(),
            source: source.to_string(),
        }],
        options.clone(),
        true,
    )
}

fn nested_scope_split_enabled(options: &DecompileOptions) -> bool {
    options.heuristic_split && options.level == RewriteLevel::Aggressive
}

pub fn unpack_files_raw(
    inputs: Vec<UnpackInput>,
    options: &DecompileOptions,
) -> Result<UnpackOutput> {
    unpack_legacy_inputs(inputs, options.clone(), true)
}

fn unpack_legacy_inputs(
    inputs: Vec<UnpackInput>,
    mut options: DecompileOptions,
    raw: bool,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one input file is required"));
    }

    let input_names = inputs
        .iter()
        .map(|input| input.filename.clone())
        .collect::<Vec<_>>();
    let single_input = inputs.len() == 1;
    if single_input {
        options.filename = input_names[0].clone();
    }
    let prepared = inputs
        .into_iter()
        .map(|input| {
            prepare_unpack_input(input.filename, input.source, options.heuristic_split, !raw)
                .map_err(DriverError::into_inner)
        })
        .collect::<Result<Vec<_>>>()?;
    let plain_single = single_input && prepared[0].detection() == PreparedInputDetection::Plain;
    let prepared_output = unpack_prepared_inputs(
        prepared,
        options.clone(),
        raw,
        nested_scope_split_enabled(&options),
    )?;
    let mut output = into_legacy_unpack_output(prepared_output, &input_names, single_input);
    if plain_single {
        let emitted_name = output.modules[0].0.clone();
        output.modules[0].0 = "module.js".to_string();
        for provenance in &mut output.provenance {
            if provenance.filename == emitted_name {
                provenance.filename = "module.js".to_string();
            }
        }
        for (filename, _) in &mut output.source_maps {
            if *filename == emitted_name {
                *filename = "module.js".to_string();
            }
        }
    }
    Ok(output)
}

fn into_legacy_unpack_output(
    output: PreparedUnpackOutput,
    input_names: &[String],
    single_input: bool,
) -> UnpackOutput {
    let mut legacy = UnpackOutput {
        warnings: output.warnings,
        detected_formats: output.detected_formats,
        ..Default::default()
    };
    for module in output.modules {
        let input = module
            .provenance
            .input
            .filter(|_| !single_input)
            .and_then(|input| input_names.get(input.index()))
            .cloned()
            .unwrap_or_default();
        legacy.provenance.push(ModuleProvenance {
            filename: module.filename.clone(),
            input,
            ranges: module.provenance.ranges,
            inspection_context_ranges: module.provenance.inspection_context_ranges,
            is_entry: module.provenance.is_entry,
        });
        if let Some(source_map) = module.source_map {
            legacy
                .source_maps
                .push((module.filename.clone(), source_map));
        }
        legacy.modules.push((module.filename, module.code));
    }
    legacy
}

fn filename_for_fallback_input(filename: &str) -> String {
    let path = std::path::Path::new(filename);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("module.js")
        .to_string()
}

pub(super) fn detect_bundle(source: &str, filename: &str) -> Result<Option<DetectedBundle>> {
    let span = tracing::info_span!("detect_bundle");
    let _enter = span.enter();

    match try_prepare_bundle(source) {
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
        module.visit_mut_with(
            &mut UnEsm::new(unresolved_mark, RewriteLevel::Standard)
                .with_current_filename(Some(filename)),
        );
        recover_late_esm_from_factory_iifes(
            &mut module,
            unresolved_mark,
            RewriteLevel::Standard,
            filename,
            LateEsmRecoveryOptions {
                smart_rename: false,
                export_rename: false,
            },
        );
        apply_fixer(&mut module)?;
        print_js(&module, cm)
    })
}

fn recover_late_esm_from_factory_iifes(
    module: &mut Module,
    unresolved_mark: Mark,
    level: RewriteLevel,
    current_filename: &str,
    options: LateEsmRecoveryOptions,
) {
    module.visit_mut_with(&mut ArrowFunction);
    module.visit_mut_with(&mut ArrowReturn);
    module.visit_mut_with(&mut UnIife::new(level));
    let pipeline_options = RulePipelineOptions::between("UnCurlyBraces", "UnEsm")
        .with_rewrite_level(level)
        .with_current_filename(current_filename);
    apply_rules(module, unresolved_mark, pipeline_options);
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

fn maybe_split_detected_bundle(
    result: DetectedBundle,
    split_nested_scope: bool,
    materialize: bool,
    render_mode: scope_hoist::ScopeHoistRenderMode,
) -> Result<DetectedBundle> {
    if !split_nested_scope && !materialize {
        return Ok(result);
    }
    let mut detected = result.materialize_prepared()?;
    if split_nested_scope {
        let excluded = detected
            .module_failures
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        detected.result = maybe_split_scope_hoisted_modules_excluding(
            detected.result,
            true,
            render_mode,
            &excluded,
        );
        detected.prepared = std::iter::repeat_with(|| None)
            .take(detected.result.modules.len())
            .collect();
    }
    Ok(detected)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
