//! Two-phase multi-module pipeline: fact collection (Phase 1) and output
//! decompilation with the cross-module late pass (Phase 2).

use anyhow::{bail, Result};
use rayon::prelude::*;
use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::Module;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::super::diagnostics::{
    collect_duplicate_declaration_warnings, collect_input_parse_warnings, collect_tdz_warnings,
    verify_output_parses,
};
use super::super::io::{
    apply_fixer, build_output_sourcemap, parse_js, parse_js_with_recovery, print_js,
    print_js_with_srcmap,
};
use super::super::types::{
    DecompileOptions, ModuleProvenance, UnpackOutput, UnpackWarning, UnpackWarningKind,
};
use super::super::unpack_cleanup::{dedup_duplicate_exports, prune_stale_local_named_exports};
use super::super::unpack_cycles::{collect_import_cycle_warnings, merge_import_cycles};
use super::dead_module::{collect_import_report, eliminate_dead_helper_modules, ImportReport};
use super::filename_recovery::{
    build_rename_map, harvest_suggested_filename, rewrite_import_sources,
};
use super::merge::{apply_numeric_rewrites, NumericRewritePlan, PreparedUnpackModule};
use super::{recover_late_esm_from_factory_iifes, LateEsmRecoveryOptions};
use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_rules, apply_rules_to_recovered_module, DeadImports, ImportDedup, RewriteLevel,
    RulePipelineOptions, SimplifySequence, UnAssignmentMerging, UnConditionals,
    UnConditionalsAssignmentOnly, UnImportRename, UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};

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
    /// Original source filename recovered from provenance markers (Sentry
    /// `data-sentry-source-file`), if any. Used at the barrier to rename the
    /// module's output file and rewrite importers' references.
    suggested_filename: Option<String>,
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
pub(super) fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<UnpackOutput> {
    let modules = modules
        .into_iter()
        .map(PreparedUnpackModule::plain)
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

pub(super) fn unpack_multi_module_with_plan(
    modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
    options: DecompileOptions,
) -> Result<UnpackOutput> {
    if options.sourcemap.is_some() {
        bail!(
            "input source maps are not supported with unpacking because extracted module coordinates differ from bundle coordinates; use --emit-source-map for output maps"
        );
    }
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

    // Stash per-module provenance (byte ranges into the original input)
    // keyed by provisional filename. Final provenance is built after dead
    // module elimination and filename recovery, so only surviving modules
    // appear with their final names.
    let provenance_by_provisional: std::collections::HashMap<String, (String, Vec<(u32, u32)>)> =
        modules
            .iter()
            .map(|prepared| {
                (
                    prepared.module.filename.clone(),
                    (
                        prepared.module.source_input.clone(),
                        prepared.module.source_ranges.clone(),
                    ),
                )
            })
            .collect();

    // Parse the sourcemap once before the loop.
    let parsed_sourcemap = options
        .sourcemap
        .as_deref()
        .map(parse_sourcemap)
        .transpose()?;
    let can_reuse_phase1_ast = parsed_sourcemap.is_none() && !options.emit_source_map;
    // Filename recovery from provenance markers is a readability rewrite, gated
    // to standard+ like other speculative recovery.
    let recover_filenames = !matches!(options.level, RewriteLevel::Minimal);
    // Dead helper-module elimination is dead-code cleanup: it relies on the
    // binding->side-effect import downgrade that only runs when DCE is on, and
    // dropping a module is structural, so gate it to standard+ as well.
    let eliminate_dead_modules =
        options.dce_mode.is_enabled() && !matches!(options.level, RewriteLevel::Minimal);

    // Phase 1: collect facts. Run the through-UnEsm normalization range on each
    // module and extract import/export facts. For normal unpacking, keep that
    // normalized AST so Phase 2 can resume after the facts barrier. Source-map
    // mode still reparses in Phase 2 because sourcemap renaming depends on the
    // original parser SourceMap.
    let collect_facts = |unpacked: &PreparedUnpackModule| -> Phase1Module {
        let globals = Globals::new();
        let (facts, prepared_parts, warning, suggested_filename) = GLOBALS.set(&globals, || {
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
                            None,
                        );
                    }
                };
            // Harvest the original filename from provenance markers before any
            // rule mutates the AST. The marker is still a props-object property
            // here (UnJsx has not run), so this does not depend on JSX recovery.
            let suggested_filename = if recover_filenames {
                harvest_suggested_filename(&module)
            } else {
                None
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
            // ESM recovery mutates the AST heavily (UnIife, factory-IIFE
            // rewrites, renames) to expose import/export declarations that
            // `collect_module_facts` reads. Phase 2 resumes from the
            // *pre-recovery* through-UnEsm barrier state and runs its own
            // recovery later at `options.level`, so it needs the unmodified
            // `module`. When the AST will be reused (no-sourcemap path), clone
            // before recovering for facts. When it won't be reused (sourcemap
            // path discards `module`), recover in place and skip the clone.
            let (facts, prepared) = if can_reuse_phase1_ast {
                let mut facts_module = module.clone();
                recover_late_esm_from_factory_iifes(
                    &mut facts_module,
                    unresolved_mark,
                    RewriteLevel::Standard,
                    LateEsmRecoveryOptions::default(),
                );
                let facts = collect_module_facts(&facts_module);
                (facts, Some((module, unresolved_mark)))
            } else {
                recover_late_esm_from_factory_iifes(
                    &mut module,
                    unresolved_mark,
                    RewriteLevel::Standard,
                    LateEsmRecoveryOptions::default(),
                );
                let facts = collect_module_facts(&module);
                (facts, None)
            };
            (facts, prepared, None, suggested_filename)
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
            suggested_filename,
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
    let mut rename_entries = Vec::with_capacity(phase1.len());
    if options.diagnostics {
        warnings.extend(cycle_warnings);
    }
    for phase1_module in phase1 {
        rename_entries.push((
            phase1_module.filename.clone(),
            phase1_module.suggested_filename,
        ));
        module_facts.insert(&phase1_module.filename, phase1_module.facts);
        prepared_modules.push(phase1_module.prepared);
        if let Some(w) = phase1_module.warning {
            warnings.push(w);
        }
    }

    // Cross-module barrier: resolve recovered filenames into a final rename
    // table. Kept separate from the fact map so the pipeline (facts, numeric
    // rewrites, namespace decomposition) keeps operating on provisional names;
    // only the final emit step swaps names and rewrites import sources.
    let rename_map = if recover_filenames {
        build_rename_map(&rename_entries)
    } else {
        std::collections::HashMap::new()
    };

    // Phase 2: output pipeline with late pass. Each module is parsed from
    // the original source only when Phase 1 failed to prepare an AST; otherwise
    // it continues from the Phase 1 normalized AST after the facts barrier.
    let facts_ref = &module_facts;
    let sm_ref = &parsed_sourcemap;
    let rename_ref = &rename_map;
    let phase2_inputs: Vec<_> = modules.into_iter().zip(prepared_modules).collect();

    let decompile_module = |(unpacked, prepared): (
        PreparedUnpackModule,
        Option<Phase1PreparedModule>,
    )|
     -> (
        String,
        String,
        Vec<UnpackWarning>,
        Option<ImportReport>,
        Option<String>,
    ) {
        let run_phase2_tail = |mut module: Module,
                               cm: Lrc<SourceMap>,
                               unresolved_mark: Mark,
                               input_parse_warnings: Vec<UnpackWarning>|
         -> Result<(
            String,
            Option<String>,
            Vec<UnpackWarning>,
            Option<ImportReport>,
        )> {
            // Late pass at the barrier
            run_reexport_consolidation(&mut module, facts_ref, Some(&unpacked.module.filename));
            run_namespace_decomposition(&mut module, facts_ref, Some(&unpacked.module.filename));
            // Preserve specifiers that were already dead at the barrier, then
            // reuse this visitor after the standalone late cleanup to remove
            // only specifiers whose last use those rewrites eliminated.
            let mut final_recovered_import_cleanup = match options.dce_mode {
                crate::DceMode::Off => None,
                crate::DceMode::TransformOnly => {
                    Some(DeadImports::preserve_currently_dead(&module))
                }
                crate::DceMode::Full => Some(DeadImports::full()),
            };
            // Late helper-through-UnReturn range.
            apply_rules_to_recovered_module(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::between("UnObjectSpread2", "UnReturn")
                    .with_dce_mode(options.dce_mode)
                    .with_rewrite_level(options.level)
                    .with_module_facts(facts_ref)
                    .with_current_filename(&unpacked.module.filename),
            );
            // Later rules can expose sequence expressions. The old unpack
            // path cleaned those by running a second full module pipeline;
            // keep only the syntax cleanup needed after the split.
            module.visit_mut_with(&mut SimplifySequence::new_with_import_semantics(
                unresolved_mark,
                options.level,
                false,
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

            if let Some(cleanup) = &mut final_recovered_import_cleanup {
                module.visit_mut_with(cleanup);
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

            // Final, isolated remap: rewrite import-source strings that point
            // at modules renamed via recovered filenames. Runs after every
            // fact-driven pass so the fact map stays keyed by provisional names.
            if !rename_ref.is_empty() {
                rewrite_import_sources(
                    &mut module,
                    &unpacked.module.filename,
                    rename_ref,
                    unresolved_mark,
                );
            }

            // Collect the dead-module-elimination report from the final AST
            // (sources are in recovered-name space after the remap above).
            let report = if eliminate_dead_modules {
                let is_helper = facts_ref
                    .get(&unpacked.module.filename)
                    .is_some_and(|facts| facts.is_helper_module);
                Some(collect_import_report(
                    &module,
                    unpacked.module.is_entry,
                    is_helper,
                ))
            } else {
                None
            };

            let final_filename = rename_ref
                .get(&unpacked.module.filename)
                .map(|s| s.as_str())
                .unwrap_or(&unpacked.module.filename);
            if !matches!(options.level, RewriteLevel::Minimal) {
                crate::rules::strip_redundant_sentry_source_file(&mut module, final_filename);
            }

            apply_fixer(&mut module)?;
            let (code, srcmap_json) = if options.emit_source_map {
                let (code, srcmap_buf) = print_js_with_srcmap(&module, cm.clone())?;
                let map_json = build_output_sourcemap(&srcmap_buf, &cm, final_filename)?;
                (code, Some(map_json))
            } else {
                (print_js(&module, cm)?, None)
            };

            if options.diagnostics {
                diag_warnings.extend(verify_output_parses(&code, &unpacked.module.filename));
            }

            Ok((code, srcmap_json, diag_warnings, report))
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
            Ok((code, srcmap_json, diag_warnings, report)) => {
                let out_filename = rename_ref
                    .get(&unpacked.module.filename)
                    .cloned()
                    .unwrap_or(unpacked.module.filename);
                (out_filename, code, diag_warnings, report, srcmap_json)
            }
            Err(e) => (
                unpacked.module.filename.clone(),
                unpacked.module.code,
                vec![UnpackWarning::new(
                    unpacked.module.filename,
                    UnpackWarningKind::DecompileFailed,
                    format!("decompile failed, preserving raw code: {e}"),
                )],
                None,
                None,
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

    // Separate source maps from the tuples before dead-module elimination.
    let mut srcmap_by_filename: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let triples_for_dead: Vec<(String, String, Vec<UnpackWarning>, Option<ImportReport>)> = triples
        .into_iter()
        .map(|(filename, code, warns, report, srcmap)| {
            if let Some(map_json) = srcmap {
                srcmap_by_filename.insert(filename.clone(), map_json);
            }
            (filename, code, warns, report)
        })
        .collect();

    let mut modules = Vec::with_capacity(triples_for_dead.len());
    if eliminate_dead_modules {
        let (kept, module_warnings) = eliminate_dead_helper_modules(triples_for_dead);
        modules = kept;
        warnings.extend(module_warnings);
    } else {
        for (filename, code, module_warnings, _report) in triples_for_dead {
            modules.push((filename, code));
            warnings.extend(module_warnings);
        }
    }
    if options.diagnostics && report_import_cycle_warnings {
        warnings.extend(collect_import_cycle_warnings(&modules));
    }

    let source_maps: Vec<(String, String)> = modules
        .iter()
        .filter_map(|(filename, _)| {
            srcmap_by_filename
                .remove(filename)
                .map(|map| (filename.clone(), map))
        })
        .collect();

    // Build final provenance from the surviving output modules, mapping
    // provisional filenames to their recovered names.  Dead helper modules
    // that were eliminated above are excluded.
    let reverse_rename: std::collections::HashMap<&str, &str> = rename_ref
        .iter()
        .map(|(prov, renamed)| (renamed.as_str(), prov.as_str()))
        .collect();
    let provenance: Vec<ModuleProvenance> = modules
        .iter()
        .filter_map(|(final_filename, _)| {
            let provisional = reverse_rename
                .get(final_filename.as_str())
                .copied()
                .unwrap_or(final_filename.as_str());
            let (input, ranges) = provenance_by_provisional.get(provisional)?;
            Some(ModuleProvenance {
                filename: final_filename.clone(),
                input: input.clone(),
                ranges: ranges.clone(),
            })
        })
        .collect();

    Ok(UnpackOutput {
        modules,
        provenance,
        warnings,
        detected_formats: Vec::new(),
        source_maps,
    })
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
    use super::super::should_merge_raw_import_cycles;
    use super::*;
    use crate::unpacker::UnpackedModule;
    use crate::DceMode;

    #[test]
    fn recovered_imports_do_not_gain_source_link_check_semantics() {
        let output = GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = parse_js(
                r#"import { recovered } from "./module.js"; void recovered;"#,
                "module.js",
                cm.clone(),
            )
            .expect("fixture should parse");
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();
            module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));

            apply_rules_to_recovered_module(
                &mut module,
                unresolved_mark,
                RulePipelineOptions::default().with_dce_mode(DceMode::TransformOnly),
            );
            apply_fixer(&mut module).expect("fixture should fix");
            print_js(&module, cm).expect("fixture should print")
        });

        assert_eq!(output, "import \"./module.js\";\n");
    }

    #[test]
    fn late_cleanup_removes_newly_dead_recovered_import_specifier() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"import { recovered } from "./module.js";
(function() {
    return void recovered;
})();
"#
            .to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                dce_mode: DceMode::TransformOnly,
                ..Default::default()
            },
        )
        .expect("fixture should decompile");
        assert_eq!(output.modules[0].1, "import \"./module.js\";\n");
    }

    #[test]
    fn late_cleanup_preserves_pre_existing_dead_recovered_import_specifier() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: r#"import { alreadyDead } from "./module.js";"#.to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                dce_mode: DceMode::TransformOnly,
                ..Default::default()
            },
        )
        .expect("fixture should decompile");
        assert_eq!(
            output.modules[0].1,
            "import { alreadyDead } from \"./module.js\";\n"
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
    fn unpack_emit_source_map_uses_phase2_parser_source_map() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "const value = input + 1;\nexport { value };".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let output = unpack_multi_module(
            modules,
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("module should decompile with source maps");

        assert_eq!(
            output.source_maps.len(),
            1,
            "unpack should emit one source map per kept module"
        );
        let sm = sourcemap::SourceMap::from_reader(output.source_maps[0].1.as_bytes())
            .expect("source map should parse");
        assert_eq!(sm.get_file(), Some("entry.js"));
        assert!(
            sm.get_token_count() > 0,
            "source map should contain generated-to-input mappings"
        );
    }

    #[test]
    fn unpack_rejects_bundle_level_input_source_map() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "export const value = 1;".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let error = unpack_multi_module(
            modules,
            DecompileOptions {
                sourcemap: Some(br#"{"version":3,"sources":[],"names":[],"mappings":""}"#.to_vec()),
                ..Default::default()
            },
        )
        .expect_err("bundle-level input maps must be rejected before module renaming");
        assert!(error.to_string().contains(
            "input source maps are not supported with unpacking because extracted module coordinates differ from bundle coordinates"
        ));
    }
}
