use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::io::{apply_fixer, parse_js, print_js};
use super::single_file::decompile;
use super::types::{DecompileOptions, UnpackInput, UnpackOutput, UnpackWarning, UnpackWarningKind};
#[cfg(test)]
use super::unpack_cleanup::hoist_late_runtime_helpers;
use super::unpack_cycles::merge_import_cycles;
#[cfg(test)]
use super::unpack_cycles::{
    collect_import_cycle_warnings, scan_local_import_dependencies, unsafe_merge_member_reason,
};
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, RewriteLevel, RulePipelineOptions, SmartRename, UnEsm,
    UnExportRename, UnIife,
};
use crate::unpacker::{
    scope_hoist, try_unpack_bundle, webpack5, BundleFormat, UnpackResult, UnpackedModule,
};

mod dead_module;
mod filename_recovery;
mod merge;
mod phases;
mod scope_split;

use merge::{
    emit_raw_modules_with_numeric_rewrites, prepare_multi_source_modules, MultiSourceModule,
    NumericRewritePlan, PreparedUnpackModule,
};
#[cfg(test)]
use phases::unpack_multi_module;
use phases::unpack_multi_module_with_plan;
use scope_split::maybe_split_scope_hoisted_modules;

pub fn unpack(source: &str, options: DecompileOptions) -> Result<UnpackOutput> {
    let span = tracing::info_span!("unpack");
    let _enter = span.enter();

    match detect_bundle(source, &options.filename)? {
        Some(result) => {
            let format = result.format;
            let result =
                maybe_split_scope_hoisted_modules(result, nested_scope_split_enabled(&options));
            let mut output = unpack_unpack_result(result, options)?;
            output.detected_formats.push(format);
            Ok(output)
        }
        None if options.heuristic_split => match scope_hoist::split_scope_hoisted(source) {
            Some(result) if result.modules.len() > 1 => {
                let mut opts = options.clone();
                opts.dce_mode = super::types::DceMode::Off;
                let mut output = unpack_unpack_result(result, opts)?;
                output.detected_formats.push(BundleFormat::ScopeHoisted);
                Ok(output)
            }
            _ => {
                let output = decompile(source, options)?;
                Ok(UnpackOutput {
                    modules: vec![("module.js".to_string(), output.code)],
                    warnings: output.warnings,
                    detected_formats: Vec::new(),
                })
            }
        },
        None => {
            let output = decompile(source, options)?;
            Ok(UnpackOutput {
                modules: vec![("module.js".to_string(), output.code)],
                warnings: output.warnings,
                detected_formats: Vec::new(),
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
    let mut detected_formats = Vec::new();
    for input in inputs {
        match detect_bundle(&input.source, &input.filename)? {
            Some(result) => {
                let format = result.format;
                if !detected_formats.contains(&format) {
                    detected_formats.push(format);
                }
                let result =
                    maybe_split_scope_hoisted_modules(result, nested_scope_split_enabled(&options));
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
                        if !detected_formats.contains(&BundleFormat::ScopeHoisted) {
                            detected_formats.push(BundleFormat::ScopeHoisted);
                        }
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

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    let mut output = unpack_multi_module_with_plan(modules, numeric_rewrite_plan, options)?;
    output.detected_formats = detected_formats;
    Ok(output)
}

/// Unpack a bundle without running the decompiler rule pipeline.
///
/// This returns raw module output after detector-specific extraction and
/// bundler-coupled normalization. Cross-module analysis and the normal
/// decompile rule pipeline are skipped.
pub fn unpack_raw(source: &str, options: &DecompileOptions) -> Result<UnpackOutput> {
    let result = detect_bundle_raw(source, &options.filename)?
        .map(|result| {
            let format = result.format;
            (
                maybe_split_scope_hoisted_modules(result, nested_scope_split_enabled(options)),
                false,
                format,
            )
        })
        .or_else(|| {
            if options.heuristic_split {
                let r = scope_hoist::split_scope_hoisted(source)?;
                if r.modules.len() > 1 {
                    Some((r, true, BundleFormat::ScopeHoisted))
                } else {
                    None
                }
            } else {
                None
            }
        });
    match result {
        Some((result, normalize_for_runnable_split, format)) => {
            let (modules, warnings) = if normalize_for_runnable_split {
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
                (output_modules, output_warnings)
            } else {
                (
                    result
                        .modules
                        .into_iter()
                        .map(|module| (module.filename, module.code))
                        .collect(),
                    Vec::new(),
                )
            };
            Ok(UnpackOutput {
                modules,
                warnings,
                detected_formats: vec![format],
            })
        }
        None => Ok(UnpackOutput {
            modules: vec![("module.js".to_string(), source.to_string())],
            warnings: Vec::new(),
            detected_formats: Vec::new(),
        }),
    }
}

fn nested_scope_split_enabled(options: &DecompileOptions) -> bool {
    options.heuristic_split && options.level == RewriteLevel::Aggressive
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
    let mut detected_formats = Vec::new();

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
                let format = result.format;
                if !detected_formats.contains(&format) {
                    detected_formats.push(format);
                }
                let result =
                    maybe_split_scope_hoisted_modules(result, nested_scope_split_enabled(options));
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
            })),
        }
    }

    let (modules, numeric_rewrite_plan) = prepare_multi_source_modules(modules);
    let mut output = emit_raw_modules_with_numeric_rewrites(modules, numeric_rewrite_plan)?;
    output.detected_formats = detected_formats;
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

fn unpack_unpack_result(result: UnpackResult, options: DecompileOptions) -> Result<UnpackOutput> {
    let allow_cycle_premerge = result.allow_cycle_premerge;
    let modules = result
        .modules
        .into_iter()
        .map(|module| PreparedUnpackModule::with_cycle_premerge(module, allow_cycle_premerge))
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

fn should_merge_raw_import_cycles(_modules: &[UnpackedModule]) -> bool {
    // Keep the raw merge hook available, but disabled for now. ESM cycles are
    // often valid, and the previous repair could undo recovered module
    // boundaries before users had a chance to inspect raw output.
    false
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
