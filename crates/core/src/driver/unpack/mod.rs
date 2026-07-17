use anyhow::{anyhow, Result};
use std::sync::Arc;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::{apply_fixer, parse_js, print_js};
use super::types::{DecompileOptions, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind};
#[cfg(test)]
use super::unpack_cleanup::hoist_late_runtime_helpers;
#[cfg(test)]
use super::unpack_cycles::merge_import_cycles;
#[cfg(test)]
use super::unpack_cycles::{
    collect_import_cycle_warnings, scan_local_import_dependencies, unsafe_merge_member_reason,
};
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

use merge::{
    emit_raw_modules_with_numeric_rewrites, input_group_for_filename, prepare_multi_source_modules,
    MultiSourceModule,
};
#[cfg(test)]
use phases::unpack_multi_module;
use phases::unpack_multi_module_with_plan;
use scope_split::maybe_split_scope_hoisted_modules;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedInputDetection {
    Bundle(BundleFormat),
    ScopeHoisted,
    Plain,
}

const PREPARED_INPUT_PREFIX: &str = "\0wakaru-input:";

pub fn prepared_input_index(source_input: &str) -> Option<usize> {
    source_input
        .strip_prefix(PREPARED_INPUT_PREFIX)?
        .parse()
        .ok()
}

fn prepared_input_label(index: usize) -> String {
    format!("{PREPARED_INPUT_PREFIX}{index}")
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
            return Ok(PreparedUnpackInput {
                filename,
                source: None,
                detection: PreparedInputDetection::Bundle(format),
                detected: Some(detected),
                scope_hoisted: None,
                plain_prepared: None,
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

    if heuristic_scope_hoist {
        if let Some(result) =
            scope_hoist::split_scope_hoisted(&source).filter(|result| result.modules.len() > 1)
        {
            return Ok(PreparedUnpackInput {
                filename,
                source: None,
                detection: PreparedInputDetection::ScopeHoisted,
                detected: None,
                scope_hoisted: Some(result),
                plain_prepared: None,
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

pub fn unpack_prepared_inputs(
    inputs: Vec<PreparedUnpackInput>,
    mut options: DecompileOptions,
    raw: bool,
    recursive_scope_hoist: bool,
) -> Result<UnpackOutput> {
    if inputs.is_empty() {
        return Err(anyhow!("at least one prepared input is required"));
    }

    let mut modules = Vec::new();
    let mut detected_formats = Vec::new();
    let mut preparation_warnings = Vec::new();
    for (input_index, input) in inputs.into_iter().enumerate() {
        let provenance_input = prepared_input_label(input_index);
        let PreparedUnpackInput {
            filename,
            source,
            detection,
            detected,
            scope_hoisted,
            plain_prepared,
        } = input;
        match detection {
            PreparedInputDetection::Bundle(format) => {
                if !detected_formats.contains(&format) {
                    detected_formats.push(format);
                }
                let detected = detected.expect("bundle detection carries prepared result");
                let chunk_ids = Arc::new(detected.chunk_ids.clone());
                let detected = if raw {
                    let result = detected.materialize()?;
                    DetectedBundle::from_result(maybe_split_scope_hoisted_modules(
                        result,
                        recursive_scope_hoist,
                    ))
                } else {
                    maybe_split_detected_bundle(
                        detected,
                        recursive_scope_hoist,
                        options.emit_source_map,
                    )?
                };
                let (result, prepared) = detected.into_parts();
                let allow_cycle_premerge = result.allow_cycle_premerge;
                let input_group = input_group_for_filename(&filename);
                modules.extend(
                    result
                        .modules
                        .into_iter()
                        .zip(prepared)
                        .map(|(module, ast)| {
                            MultiSourceModule::detected_with_ast_from_source(
                                module,
                                ast,
                                chunk_ids.clone(),
                                filename.clone(),
                                provenance_input.clone(),
                                input_group.clone(),
                                allow_cycle_premerge,
                            )
                        }),
                );
            }
            PreparedInputDetection::ScopeHoisted => {
                if !detected_formats.contains(&BundleFormat::ScopeHoisted) {
                    detected_formats.push(BundleFormat::ScopeHoisted);
                }
                let result = scope_hoisted.expect("scope-hoist detection carries result");
                modules.extend(result.modules.into_iter().map(|mut module| {
                    module.source_input = provenance_input.clone();
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
                    MultiSourceModule::fallback(module)
                }));
            }
            PreparedInputDetection::Plain => {
                let source = source.expect("plain input retains source");
                let source_len = source.len() as u32;
                modules.push(MultiSourceModule::fallback_with_ast(
                    UnpackedModule {
                        id: filename.clone(),
                        is_entry: false,
                        filename: filename_for_fallback_input(&filename),
                        source_ranges: vec![(0, source_len)],
                        source_input: provenance_input,
                        generated_source_map: Vec::new(),
                        code: source,
                    },
                    (!raw).then_some(plain_prepared).flatten(),
                ));
            }
        }
    }

    if !raw && detected_formats == [BundleFormat::ScopeHoisted] && modules.len() > 1 {
        options.dce_mode = super::types::DceMode::Off;
    }
    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    let mut output = if raw {
        emit_raw_modules_with_numeric_rewrites(modules, numeric_rewrite_plan)?
    } else {
        unpack_multi_module_with_plan(modules, numeric_rewrite_plan, options.clone())?
    };
    output.warnings.splice(0..0, preparation_warnings);
    output.detected_formats = detected_formats;
    Ok(output)
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
    let mut output = unpack_prepared_inputs(
        prepared,
        options.clone(),
        raw,
        nested_scope_split_enabled(&options),
    )?;

    for provenance in &mut output.provenance {
        if let Some(index) = prepared_input_index(&provenance.input) {
            provenance.input = if single_input {
                String::new()
            } else {
                input_names.get(index).cloned().unwrap_or_default()
            };
        }
    }
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
        apply_fixer(&mut module)?;
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

fn maybe_split_detected_bundle(
    result: DetectedBundle,
    split_nested_scope: bool,
    materialize: bool,
) -> Result<DetectedBundle> {
    if !split_nested_scope && !materialize {
        return Ok(result);
    }
    let result = result.materialize()?;
    let result = maybe_split_scope_hoisted_modules(result, split_nested_scope);
    Ok(DetectedBundle::from_result(result))
}

#[cfg(test)]
fn should_merge_raw_import_cycles(_modules: &[UnpackedModule]) -> bool {
    // Keep the raw merge hook available, but disabled for now. ESM cycles are
    // often valid, and the previous repair could undo recovered module
    // boundaries before users had a chance to inspect raw output.
    false
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
