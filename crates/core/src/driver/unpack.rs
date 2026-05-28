use anyhow::{anyhow, Result};
use rayon::prelude::*;
use swc_core::common::{sync::Lrc, Mark, SourceMap, GLOBALS};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::visit::VisitMutWith;

use super::diagnostics::{
    collect_duplicate_declaration_warnings, collect_input_parse_warnings, collect_tdz_warnings,
    verify_output_parses,
};
use super::io::{parse_js, parse_js_with_recovery, print_js};
use super::single_file::decompile;
use super::types::{DecompileOptions, UnpackOutput, UnpackWarning, UnpackWarningKind};
use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_rules, ArrowFunction, ArrowReturn, ImportDedup, RewriteLevel, RulePipelineOptions,
    SimplifySequence, UnAssignmentMerging, UnConditionals, UnEsm, UnExportRename, UnIife,
    UnImportRename, UnObjectSpread, UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::unpacker::{scope_hoist, try_unpack_bundle, UnpackResult};

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
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
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
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
                );

                // Late pass at the barrier
                run_reexport_consolidation(&mut module, facts_ref);
                run_namespace_decomposition(&mut module, facts_ref);
                module.visit_mut_with(&mut UnObjectSpread::new_with_facts(facts_ref));

                // Stage 3+
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::between("UnTemplateLiteral", "UnReturn")
                        .with_dead_code_elimination(options.dead_code_elimination)
                        .with_rewrite_level(options.level)
                        .with_module_facts(facts_ref),
                );
                // Later rules can expose sequence expressions. Keep the narrow
                // syntax cleanup without restoring the old full second pass.
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
                module.visit_mut_with(&mut UnConditionals);

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
