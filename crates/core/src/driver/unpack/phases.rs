//! Two-phase multi-module pipeline: fact collection (Phase 1) and output
//! decompilation with the cross-module late pass (Phase 2).

use anyhow::{bail, Result};
use rayon::prelude::*;
use swc_core::common::{sync::Lrc, Globals, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Expr, ExprStmt, Ident, IdentName, MemberExpr, MemberProp,
    Module, ModuleItem, ObjectLit, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::VisitMutWith;

use super::super::diagnostics::{collect_input_parse_warnings, collect_output_diagnostics};
use super::super::io::{
    apply_fixer, build_output_sourcemap, parse_js, parse_js_with_recovery, print_js,
    print_js_with_srcmap, ParseDiagnostic,
};
use super::super::types::{
    DecompileOptions, PreparedInputId, PreparedModuleOutput, PreparedModuleProvenance,
    PreparedUnpackOutput, UnpackWarning, UnpackWarningKind,
};
use super::super::unpack_cleanup::{dedup_duplicate_exports, prune_stale_local_named_exports};
use super::super::unpack_cycles::collect_import_cycle_warnings;
use super::dead_module::{collect_import_report, eliminate_dead_helper_modules, ImportReport};
use super::filename_recovery::{
    build_rename_map, harvest_suggested_filename, rewrite_import_sources,
};
use super::merge::{
    apply_filename_rewrites, apply_numeric_rewrites, NumericRewritePlan, PreparedUnpackModule,
};
use super::webpack_commonjs_runtime::normalize_webpack_commonjs_runtime;
use super::{recover_late_esm_from_factory_iifes, LateEsmRecoveryOptions};
use crate::commonjs_default_object_composition::{
    run_commonjs_default_object_composition, CommonJsDefaultObjectCompositionPlan,
};
use crate::facts::{
    collect_commonjs_default_attached_properties, collect_commonjs_default_object,
    collect_module_facts, ModuleFactsMap,
};
use crate::namespace_decomposition::run_namespace_decomposition;
use crate::provider_import_repair::run_provider_import_repair;
use crate::provider_namespace_repair::run_provider_namespace_repair;
use crate::reexport_consolidation::run_reexport_consolidation;
use crate::rules::{
    apply_rules, apply_rules_to_recovered_module, DeadImports, ImportDedup, RewriteLevel,
    RulePipelineOptions, SimplifySequence, UnAssignmentMerging, UnConditionals,
    UnConditionalsAssignmentOnly, UnImportRename, UnOptionalChaining,
};
use crate::sourcemap_rename::{apply_sourcemap_renames, parse_sourcemap};
use crate::synthetic_import_cleanup::downgrade_unused_synthetic_imports;
use crate::unpacker::DetectedModuleFailure;

fn detector_failure_warning(filename: &str, failure: DetectedModuleFailure) -> UnpackWarning {
    match failure {
        DetectedModuleFailure::WebpackRuntimeParameterReuse => UnpackWarning::new(
            filename,
            UnpackWarningKind::WebpackFactoryRecoveryFailed,
            "webpack factory runtime-parameter reuse could not be normalized; preserving the opaque factory body",
        ),
    }
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
    input_parse_warnings: Vec<UnpackWarning>,
    /// Original source filename recovered from provenance markers (Sentry
    /// `data-sentry-source-file`), if any. Used at the barrier to rename the
    /// module's output file and rewrite importers' references.
    suggested_filename: Option<String>,
}

/// Restore the default object that webpack creates before invoking an empty
/// CommonJS factory. The detector body is intentionally left empty for raw
/// output; only the normal ESM-recovery pipeline receives this synthetic fact.
fn restore_empty_webpack_factory_default(
    module: &mut Module,
    unresolved_mark: Mark,
    enabled: bool,
) {
    if !enabled || !module.body.is_empty() {
        return;
    }

    let module_ident = Ident::new(
        "module".into(),
        DUMMY_SP,
        SyntaxContext::empty().apply_mark(unresolved_mark),
    );
    module.body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(module_ident)),
                prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
            })),
            right: Box::new(Expr::Object(ObjectLit {
                span: DUMMY_SP,
                props: Vec::new(),
            })),
        })),
    })));
}

/// Multi-module unpack with cross-module late pass.
///
/// Phase 1: obtain a resolved AST + through-UnEsm range + fact recovery/collection
/// Phase 2: resume the retained AST + late pass + UnTemplateLiteral-through-UnReturn range
///
/// Normal unpack retains each Phase 1 AST together with its `Globals` and
/// unresolved mark across the facts barrier. Webpack5 detectors can provide an
/// already-resolved, bundler-normalized AST, avoiding their intermediate emit
/// and this phase's parse/resolver. Source-map mode deliberately materializes
/// detector ASTs and uses the parser path so output mappings keep parser-owned
/// source coordinates.
///
/// # Best-effort semantics
///
/// Individual extracted modules that fail to parse are preserved as raw code
/// rather than aborting the entire unpack. The extraction process can
/// produce module bodies that are not valid standalone JS (e.g. incomplete
/// slicing, runtime wrapper residue). Hard-failing on those would discard
/// all other successfully extracted modules, which is worse for both
/// interactive and automated users. Failures are reported via
/// `PreparedUnpackOutput::warnings` so callers can surface them without silent
/// swallowing.
///
/// Both phases run via rayon. On targets without threading support, Rayon falls
/// back to sequential execution.
#[cfg(test)]
pub(super) fn unpack_multi_module(
    modules: Vec<crate::unpacker::UnpackedModule>,
    options: DecompileOptions,
) -> Result<PreparedUnpackOutput> {
    let modules = modules
        .into_iter()
        .map(PreparedUnpackModule::plain)
        .collect();
    unpack_multi_module_with_plan(modules, NumericRewritePlan::default(), options)
}

pub(super) fn unpack_multi_module_with_plan(
    mut modules: Vec<PreparedUnpackModule>,
    numeric_rewrite_plan: NumericRewritePlan,
    options: DecompileOptions,
) -> Result<PreparedUnpackOutput> {
    if options.sourcemap.is_some() {
        bail!(
            "input source maps are not supported with unpacking because extracted module coordinates differ from bundle coordinates; use --emit-source-map for output maps"
        );
    }
    let span = tracing::info_span!("unpack_multi_module", count = modules.len());
    let _enter = span.enter();
    let report_import_cycle_warnings = modules
        .iter()
        .all(|module| module.report_import_cycle_warnings);
    let reserved_public_paths = modules
        .iter()
        .filter(|module| module.reserved_public_path)
        .map(|module| module.module.filename.clone())
        .collect::<std::collections::HashSet<_>>();

    // Stash per-module provenance (byte ranges into the original input)
    // keyed by provisional filename. Final provenance is built after dead
    // module elimination and filename recovery, so only surviving modules
    // appear with their final names.
    let provenance_by_provisional: std::collections::HashMap<
        String,
        (
            Option<PreparedInputId>,
            Vec<(u32, u32)>,
            Vec<(u32, u32)>,
            bool,
        ),
    > = modules
        .iter()
        .map(|prepared| {
            (
                prepared.module.filename.clone(),
                (
                    prepared.input,
                    prepared.module.source_ranges.clone(),
                    prepared.module.inspection_context_ranges.clone(),
                    prepared.module.is_entry,
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
    let collect_facts = |unpacked: &mut PreparedUnpackModule| -> Phase1Module {
        if let Some(failure) = unpacked.detector_failure {
            // The factory loader's later lifetime is unproven. Running any
            // require-aware transform or fact collector over this body could
            // turn an ordinary local call into a fabricated module edge.
            unpacked.prepared = None;
            return Phase1Module {
                filename: unpacked.module.filename.clone(),
                facts: crate::facts::ModuleFacts::default(),
                prepared: None,
                warning: Some(detector_failure_warning(&unpacked.module.filename, failure)),
                input_parse_warnings: Vec::new(),
                suggested_filename: None,
            };
        }
        let (globals, prepared_input, input_parse_warnings) = match unpacked.prepared.take() {
            Some(prepared) => {
                let filename = unpacked.module.filename.clone();
                let errors = prepared
                    .recoverable_parse_errors
                    .into_iter()
                    .map(|error| ParseDiagnostic {
                        filename: filename.clone(),
                        line: error.line,
                        column: error.column,
                        message: error.message,
                    })
                    .collect::<Vec<_>>();
                let warnings = collect_input_parse_warnings(&errors);
                (
                    prepared.globals,
                    Some((prepared.module, prepared.unresolved_mark)),
                    warnings,
                )
            }
            None => (Globals::new(), None, Vec::new()),
        };
        let (facts, prepared_parts, warning, suggested_filename) = GLOBALS.set(&globals, || {
            let (mut module, unresolved_mark) = match prepared_input {
                Some(prepared) => prepared,
                None => {
                    let cm: Lrc<SourceMap> = Default::default();
                    let mut module = {
                        let span = tracing::info_span!("phase1: parse");
                        let _enter = span.enter();
                        match parse_js(
                            &unpacked.module.code,
                            &unpacked.module.filename,
                            cm.clone(),
                        ) {
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
                        }
                    };
                    let unresolved_mark = {
                        let span = tracing::info_span!("phase1: resolver");
                        let _enter = span.enter();
                        let unresolved_mark = Mark::new();
                        let top_level_mark = Mark::new();
                        module.visit_mut_with(&mut resolver(
                            unresolved_mark,
                            top_level_mark,
                            false,
                        ));
                        unresolved_mark
                    };
                    (module, unresolved_mark)
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
            restore_empty_webpack_factory_default(
                &mut module,
                unresolved_mark,
                unpacked.implicit_commonjs_default_object,
            );
            apply_filename_rewrites(
                &mut module,
                unresolved_mark,
                unpacked.filename_rewrite.as_ref(),
            );
            apply_numeric_rewrites(
                &mut module,
                unresolved_mark,
                unpacked.numeric_rewrite.as_ref(),
                &numeric_rewrite_plan,
            );
            normalize_webpack_commonjs_runtime(
                &mut module,
                unresolved_mark,
                unpacked.webpack_commonjs_runtime,
                unpacked.webpack_numeric_module_id,
                unpacked.webpack_legacy_module_i,
            );
            let commonjs_default_object =
                collect_commonjs_default_object(&module, unresolved_mark);
            let commonjs_default_attached_properties =
                collect_commonjs_default_attached_properties(&module, unresolved_mark);
            {
                let span = tracing::info_span!("phase1: rules");
                let _enter = span.enter();
                apply_rules(
                    &mut module,
                    unresolved_mark,
                    RulePipelineOptions::until("UnEsm"),
                );
            }
            // ESM recovery mutates the AST heavily (UnIife, factory-IIFE
            // rewrites, renames) to expose import/export declarations that
            // `collect_module_facts` reads. Phase 2 resumes from the
            // *pre-recovery* through-UnEsm barrier state and runs its own
            // recovery later at `options.level`, so it needs the unmodified
            // `module`. When the AST will be reused (no-sourcemap path), clone
            // before recovering for facts. When it won't be reused (sourcemap
            // path discards `module`), recover in place and skip the clone.
            let (mut facts, prepared) = if can_reuse_phase1_ast {
                let mut facts_module = module.clone();
                {
                    let span = tracing::info_span!("phase1: fact recovery");
                    let _enter = span.enter();
                    recover_late_esm_from_factory_iifes(
                        &mut facts_module,
                        unresolved_mark,
                        RewriteLevel::Standard,
                        LateEsmRecoveryOptions::default(),
                    );
                }
                let facts = collect_module_facts(&facts_module);
                (facts, Some((module, unresolved_mark)))
            } else {
                {
                    let span = tracing::info_span!("phase1: fact recovery");
                    let _enter = span.enter();
                    recover_late_esm_from_factory_iifes(
                        &mut module,
                        unresolved_mark,
                        RewriteLevel::Standard,
                        LateEsmRecoveryOptions::default(),
                    );
                }
                let facts = collect_module_facts(&module);
                (facts, None)
            };
            facts.commonjs_default_object = commonjs_default_object;
            facts.commonjs_default_attached_properties =
                commonjs_default_attached_properties;
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
            input_parse_warnings,
            suggested_filename,
        }
    };

    let phase1: Vec<_> = {
        let span = tracing::info_span!("phase1_collect_facts");
        let _enter = span.enter();
        modules.par_iter_mut().map(collect_facts).collect()
    };

    let mut module_facts = ModuleFactsMap::new();
    let mut prepared_modules = Vec::with_capacity(phase1.len());
    let mut prepared_parse_warnings = Vec::with_capacity(phase1.len());
    let mut warnings = Vec::new();
    let mut rename_entries = Vec::with_capacity(phase1.len());
    for phase1_module in phase1 {
        rename_entries.push((
            phase1_module.filename.clone(),
            phase1_module.suggested_filename,
        ));
        module_facts.insert(&phase1_module.filename, phase1_module.facts);
        prepared_modules.push(phase1_module.prepared);
        prepared_parse_warnings.push(phase1_module.input_parse_warnings);
        if let Some(w) = phase1_module.warning {
            warnings.push(w);
        }
    }

    let commonjs_default_object_composition_plan =
        CommonJsDefaultObjectCompositionPlan::build(&module_facts);

    // Cross-module barrier: resolve recovered filenames into a final rename
    // table. Kept separate from the fact map so the pipeline (facts, numeric
    // rewrites, namespace decomposition) keeps operating on provisional names;
    // only the final emit step swaps names and rewrites import sources.
    let rename_map = if recover_filenames {
        build_rename_map(&rename_entries, &reserved_public_paths)
    } else {
        std::collections::HashMap::new()
    };

    // Phase 2: output pipeline with late pass. Each module is parsed from
    // the original source only when Phase 1 failed to prepare an AST; otherwise
    // it continues from the Phase 1 normalized AST after the facts barrier.
    let facts_ref = &module_facts;
    let composition_plan_ref = &commonjs_default_object_composition_plan;
    let sm_ref = &parsed_sourcemap;
    let rename_ref = &rename_map;
    let phase2_inputs: Vec<_> = modules
        .into_iter()
        .zip(prepared_modules)
        .zip(prepared_parse_warnings)
        .map(|((module, prepared), warnings)| (module, prepared, warnings))
        .collect();

    let decompile_module = |(unpacked, prepared, prepared_parse_warnings): (
        PreparedUnpackModule,
        Option<Phase1PreparedModule>,
        Vec<UnpackWarning>,
    )|
     -> (
        String,
        String,
        Vec<UnpackWarning>,
        Option<ImportReport>,
        Option<String>,
    ) {
        if unpacked.detector_failure.is_some() {
            return (
                unpacked.module.filename,
                unpacked.module.code,
                Vec::new(),
                None,
                None,
            );
        }
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
            let rules_span = tracing::info_span!("phase2: rules");
            let rules_enter = rules_span.enter();
            // Late pass at the barrier
            run_commonjs_default_object_composition(
                &mut module,
                composition_plan_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            run_provider_import_repair(&mut module, facts_ref, Some(&unpacked.module.filename));
            run_provider_namespace_repair(
                &mut module,
                facts_ref,
                Some(&unpacked.module.filename),
                unresolved_mark,
            );
            run_reexport_consolidation(&mut module, facts_ref, Some(&unpacked.module.filename));
            run_namespace_decomposition(&mut module, facts_ref, Some(&unpacked.module.filename));
            downgrade_unused_synthetic_imports(&mut module);
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
            drop(rules_enter);
            drop(rules_span);

            let mut diag_warnings = input_parse_warnings;

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

            {
                let span = tracing::info_span!("phase2: fixer");
                let _enter = span.enter();
                apply_fixer(&mut module)?;
            }
            let (code, srcmap_json) = if options.emit_source_map {
                let (code, srcmap_buf) = {
                    let span = tracing::info_span!("phase2: emit");
                    let _enter = span.enter();
                    print_js_with_srcmap(&module, cm.clone())?
                };
                let map_json = build_output_sourcemap(&srcmap_buf, &cm, final_filename)?;
                (code, Some(map_json))
            } else {
                let code = {
                    let span = tracing::info_span!("phase2: emit");
                    let _enter = span.enter();
                    print_js(&module, cm)?
                };
                (code, None)
            };

            if options.diagnostics {
                diag_warnings.extend(collect_output_diagnostics(&code, &unpacked.module.filename));
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
                run_phase2_tail(module, cm, unresolved_mark, prepared_parse_warnings)
            })
        } else {
            GLOBALS.set(&Default::default(), || {
                let cm: Lrc<SourceMap> = Default::default();
                let parsed = {
                    let span = tracing::info_span!("phase2: parse");
                    let _enter = span.enter();
                    parse_js_with_recovery(
                        &unpacked.module.code,
                        &unpacked.module.filename,
                        cm.clone(),
                    )?
                };
                let mut module = parsed.module;
                let unresolved_mark = {
                    let span = tracing::info_span!("phase2: resolver");
                    let _enter = span.enter();
                    let unresolved_mark = Mark::new();
                    let top_level_mark = Mark::new();
                    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
                    unresolved_mark
                };
                restore_empty_webpack_factory_default(
                    &mut module,
                    unresolved_mark,
                    unpacked.implicit_commonjs_default_object,
                );
                apply_filename_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.filename_rewrite.as_ref(),
                );
                apply_numeric_rewrites(
                    &mut module,
                    unresolved_mark,
                    unpacked.numeric_rewrite.as_ref(),
                    &numeric_rewrite_plan,
                );
                normalize_webpack_commonjs_runtime(
                    &mut module,
                    unresolved_mark,
                    unpacked.webpack_commonjs_runtime,
                    unpacked.webpack_numeric_module_id,
                    unpacked.webpack_legacy_module_i,
                );

                // Through-UnEsm range.
                {
                    let span = tracing::info_span!("phase2: early rules");
                    let _enter = span.enter();
                    apply_rules(
                        &mut module,
                        unresolved_mark,
                        RulePipelineOptions::until("UnEsm"),
                    );
                }

                let input_parse_warnings = collect_input_parse_warnings(&parsed.recoverable_errors);
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

    // Build final provenance from the surviving output modules, mapping
    // provisional filenames to their recovered names.  Dead helper modules
    // that were eliminated above are excluded.
    let reverse_rename: std::collections::HashMap<&str, &str> = rename_ref
        .iter()
        .map(|(prov, renamed)| (renamed.as_str(), prov.as_str()))
        .collect();
    let modules = modules
        .into_iter()
        .map(|(final_filename, code)| {
            let provisional = reverse_rename
                .get(final_filename.as_str())
                .copied()
                .unwrap_or(final_filename.as_str());
            let (input, ranges, inspection_context_ranges, is_entry) = provenance_by_provisional
                .get(provisional)
                .expect("every surviving module retains its provenance");
            let source_map = srcmap_by_filename.remove(&final_filename);
            PreparedModuleOutput {
                filename: final_filename,
                code,
                source_map,
                provenance: PreparedModuleProvenance {
                    input: *input,
                    ranges: ranges.clone(),
                    inspection_context_ranges: inspection_context_ranges.clone(),
                    is_entry: *is_entry,
                },
            }
        })
        .collect();

    Ok(PreparedUnpackOutput {
        modules,
        warnings,
        detected_formats: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_tracing::record_spans;
    use crate::unpacker::UnpackedModule;
    use crate::{validate_output_modules, DceMode, OutputFindingKind};

    fn validate_prepared_output(output: &PreparedUnpackOutput) -> Vec<crate::OutputFinding> {
        let modules = output
            .modules
            .iter()
            .map(|module| (module.filename.clone(), module.code.clone()))
            .collect::<Vec<_>>();
        validate_output_modules(&modules)
    }

    #[test]
    fn profiler_reports_phase_operation_boundaries() {
        let modules = vec![UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code: "export const value = 1;".to_string(),
            filename: "entry.js".to_string(),
            ..Default::default()
        }];

        let (output, spans) = record_spans(|| {
            unpack_multi_module(modules, DecompileOptions::default())
                .expect("profiled fixture should decompile")
        });

        assert_eq!(output.modules.len(), 1);
        for expected in [
            "phase1: parse",
            "phase1: resolver",
            "phase1: rules",
            "phase1: fact recovery",
            "phase2: rules",
            "phase2: fixer",
            "phase2: emit",
        ] {
            assert!(
                spans.iter().any(|name| name == expected),
                "missing {expected:?} in {spans:?}"
            );
        }
        assert!(
            !spans.iter().any(|name| name == "phase2: parse"),
            "normal unpack should reuse the Phase 1 AST"
        );
    }

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
        assert_eq!(output.modules[0].code, "import \"./module.js\";\n");
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
            output.modules[0].code,
            "import { alreadyDead } from \"./module.js\";\n"
        );
    }

    #[test]
    fn scope_split_cycles_do_not_emit_diagnostic_warnings() {
        let modules = vec![
            PreparedUnpackModule::with_cycle_warnings(
                UnpackedModule {
                    id: "a".to_string(),
                    is_entry: true,
                    code: r#"import { b } from "./b.js"; export const a = b + 1;"#.to_string(),
                    filename: "entry.js".to_string(),
                    ..Default::default()
                },
                false,
            ),
            PreparedUnpackModule::with_cycle_warnings(
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
        let code = &output.modules[0].code;
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
        let code = &output.modules[0].code;
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
    fn lowered_interop_residue_does_not_block_named_import_recovery() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.transform = function(value) { return value + 1; };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
function interopRequireDefault(value) {
    return value && value.__esModule ? value : { default: value };
}
var provider = require("./provider.js");
interopRequireDefault(provider);
module.exports = function(value) { return provider.transform(value); };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("interop residue fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert_eq!(findings, vec![]);

        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import { transform }") && !consumer.contains("provider;"),
            "an inert lowered interop read must not retain a missing default import:\n{consumer}"
        );
    }

    #[test]
    fn named_only_provider_repairs_synthetic_default_to_namespace_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
exports.alpha = 1;
exports.beta = 2;
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var provider = require("./provider.js");
function read(alpha, beta) {
    return provider.alpha + provider.beta + alpha + beta;
}
module.exports = read;
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("named-only provider fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "colliding local names should retain a valid namespace fallback:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("namespace repair should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "namespace repair must preserve emitted source maps"
        );
    }

    #[test]
    fn named_only_provider_repairs_a_transparent_synthetic_import_alias() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
const value = provider.alpha + provider.beta;
provider = { alpha: 3 };
module.exports = value + provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a transparent alias should preserve namespace semantics");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as imported from \"./provider.js\";"),
            "the synthesized edge should become a namespace import:\n{consumer}"
        );
        assert!(
            consumer.contains("provider = {") && consumer.contains("provider.alpha"),
            "the reassigned alias must keep its later, non-namespace lifetime:\n{consumer}"
        );
    }

    #[test]
    fn named_only_provider_keeps_namespace_member_mutation_fail_closed() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
provider.alpha = 2;
module.exports = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a namespace mutation should remain printable");
        let findings = validate_prepared_output(&output);
        assert!(findings.iter().any(|finding| {
            finding.kind == OutputFindingKind::MissingImportedName
                && finding.filename == "consumer.js"
        }));
    }

    #[test]
    fn named_only_provider_keeps_conditional_alias_replacement_fail_closed() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var imported = require("./provider.js");
let provider = imported;
if (replaceProvider) provider = { alpha: 2 };
module.exports = provider.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("a conditional alias replacement should remain printable");
        let findings = validate_prepared_output(&output);
        assert!(findings.iter().any(|finding| {
            finding.kind == OutputFindingKind::MissingImportedName
                && finding.filename == "consumer.js"
        }));
    }

    #[test]
    fn named_only_provider_namespace_supports_enumeration_and_copy_sources() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
var copy = {};
Object.assign(copy, provider);
module.exports = Object.keys(provider).join(",") + copy.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("namespace enumeration fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "whole-namespace enumeration and copy must use a namespace import:\n{consumer}"
        );
    }

    #[test]
    fn recovered_export_star_surface_repairs_downstream_namespace_copy() {
        let modules = vec![
            UnpackedModule {
                id: "source".to_string(),
                code: "exports.alpha = 1; exports.beta = 2;".to_string(),
                filename: "source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "facade".to_string(),
                code: r#"
var source = require("./source.js");
Object.keys(source).forEach(function(key) {
    key !== "default" && key !== "__esModule" &&
        (key in exports && exports[key] === source[key] ||
            (exports[key] = source[key]));
});
"#
                .to_string(),
                filename: "facade.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var facade = require("./facade.js");
var copy = {};
Object.assign(copy, facade);
module.exports = copy.alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("recovered export-star fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let facade = output
            .modules
            .iter()
            .find(|module| module.filename == "facade.js")
            .map(|module| module.code.as_str())
            .expect("expected facade module");
        assert_eq!(facade, "export * from \"./source.js\";\n");
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as facade from \"./facade.js\";"),
            "the downstream copy must consume the recovered namespace surface:\n{consumer}"
        );
    }

    #[test]
    fn export_star_provider_supports_synthetic_namespace_require() {
        let modules = vec![
            UnpackedModule {
                id: "source".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "provider".to_string(),
                code: "export * from \"./source.js\";".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
module.exports = Object.keys(provider);
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("export-star namespace fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import * as provider from \"./provider.js\";"),
            "an export-star surface is valid through a namespace import:\n{consumer}"
        );
    }

    #[test]
    fn authored_default_import_is_not_provider_repaired() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
import provider from "./provider.js";
console.log(Object.keys(provider));
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored import fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "provider repair must not reinterpret authored default imports: {findings:#?}"
        );
    }

    #[test]
    fn provider_namespace_repair_rejects_esmodule_meta_observation() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
module.exports = provider.__esModule;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("namespace metadata fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "observing CommonJS interop metadata must keep namespace repair fail closed: {findings:#?}"
        );
    }

    #[test]
    fn getter_map_default_provider_supports_synthetic_default_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
((target, getters) => {
    for (const key in getters) {
        Object.defineProperty(target, key, {
            enumerable: true,
            get: getters[key]
        });
    }
})(exports, {
    dim() { return dim; },
    default() { return logger; }
});
function dim(value) { return value; }
const logger = {
    warn(value) { console.warn(value); }
};
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var logger = require("./provider.js");
module.exports = function(value) { return logger.warn(value); };
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("getter-map default fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let provider = output
            .modules
            .iter()
            .find(|module| module.filename == "provider.js")
            .map(|module| module.code.as_str())
            .expect("expected provider module");
        assert!(
            provider.contains("export { logger as default };")
                && !provider.contains("Object.defineProperty"),
            "the provider should expose the recovered live default:\n{provider}"
        );
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import logger from \"./provider.js\";"),
            "the proven provider default should satisfy the synthetic require:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("getter-map default should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "getter-map default recovery must preserve emitted source maps"
        );
    }

    #[test]
    fn unused_synthetic_require_becomes_side_effect_import() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: "exports.alpha = 1;".to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var unused = require("./provider.js");
after();
module.exports = 42;
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("unused require fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import \"./provider.js\";"),
            "provider evaluation must survive as a side-effect import:\n{consumer}"
        );
        assert!(
            !consumer.contains("import unused") && !consumer.contains("var unused"),
            "the unused guessed default binding should be removed:\n{consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("unused require cleanup should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "side-effect import recovery must preserve emitted source maps"
        );
    }

    #[test]
    fn unused_synthetic_default_is_removed_from_mixed_import() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var unused = require("./provider.js");
var alpha = require("./provider.js").alpha;
module.exports = alpha;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("mixed import fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import { alpha }") && !consumer.contains("import unused"),
            "only the used named binding should remain:\n{consumer}"
        );
    }

    #[test]
    fn unused_authored_import_is_not_synthetic_cleanup() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.alpha = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
import missing from "./provider.js";
export const result = 1;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored unused import fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "cleanup must not reinterpret an authored dead import: {findings:#?}"
        );
    }

    #[test]
    fn substantive_namespace_use_still_blocks_named_import_recovery() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.transform = function(value) { return value + 1; };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
function interopRequireDefault(value) {
    return value && value.__esModule ? value : { default: value };
}
var provider = require("./provider.js");
interopRequireDefault(provider);
observe(provider);
module.exports = function(value) { return provider.transform(value); };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("substantive namespace fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "a substantive whole-object use must keep the namespace repair fail closed: {findings:#?}"
        );
    }

    #[test]
    fn provider_facts_capture_missing_properties_from_proven_empty_default_objects() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "empty-stub".to_string(),
                    code: "module.exports = {};".to_string(),
                    filename: "empty-stub.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var existsSync = require("./empty-stub.js").existsSync;
after();
module.exports = function(path) {
    return Boolean(existsSync) && existsSync(path);
};
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("proven empty default-object fixture should decompile");
            assert_eq!(validate_prepared_output(&output), vec![]);

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                !consumer.contains("import { existsSync"),
                "a CommonJS property read must not remain a named import:\n{consumer}"
            );
            assert!(
                consumer.contains(".existsSync"),
                "the property must be captured from the proven default object:\n{consumer}"
            );
            let before = consumer
                .find("before();")
                .expect("leading effect should remain");
            let capture = consumer[before..]
                .find(".existsSync")
                .map(|offset| before + offset)
                .expect("property capture should remain");
            let after = consumer
                .find("after();")
                .expect("trailing effect should remain");
            assert!(
                before < capture && capture < after,
                "the property read must remain at its original require position:\n{consumer}"
            );
            if emit_source_map {
                assert!(
                    output
                        .modules
                        .iter()
                        .all(|module| module.source_map.is_some()),
                    "provider repair must preserve emitted source maps"
                );
            }
        }
    }

    #[test]
    fn provider_facts_capture_proven_callable_default_properties() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
api.Rule = class Rule {};
module.exports = api;
api.default = api;
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var parse = require("./provider.js").parse;
var Rule = require("./provider.js").Rule;
after();
module.exports = [parse("value"), new Rule()];
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("proven callable-property fixture should decompile");
            assert_eq!(validate_prepared_output(&output), vec![]);

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                !consumer.contains("import { parse") && !consumer.contains("Rule } from"),
                "attached CommonJS properties must not remain guessed named imports:\n{consumer}"
            );
            assert!(
                (consumer.contains(".parse") && consumer.contains(".Rule"))
                    || consumer.contains("{ parse, Rule }"),
                "attached properties must be captured from the callable default:\n{consumer}"
            );
            let before = consumer
                .find("before();")
                .expect("leading effect should remain");
            let parse_capture = consumer
                .find(".parse")
                .or_else(|| consumer.find("{ parse"))
                .expect("parse capture should remain");
            let rule_capture = consumer[parse_capture..]
                .find("Rule")
                .map(|offset| parse_capture + offset)
                .expect("Rule capture should remain");
            let after = consumer
                .find("after();")
                .expect("trailing effect should remain");
            assert!(
                before < parse_capture && parse_capture < rule_capture && rule_capture < after,
                "callable property reads must stay at their original require positions:\n{consumer}"
            );
            if emit_source_map {
                assert!(
                    output
                        .modules
                        .iter()
                        .all(|module| module.source_map.is_some()),
                    "callable property repair must preserve emitted source maps"
                );
            }
        }
    }

    #[test]
    fn provider_facts_reject_a_conditionally_rewritten_callable_default() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
module.exports = api;
if (globalThis.legacy) { module.exports = globalThis.wrap(api); }
"#
                    .to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: r#"
var parse = require("./provider.js").parse;
module.exports = function(value) { return parse(value); };
"#
                    .to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        for emit_source_map in [false, true] {
            let output = unpack_multi_module(
                modules(),
                DecompileOptions {
                    emit_source_map,
                    ..Default::default()
                },
            )
            .expect("conditional callable-default fixture should decompile");
            let findings = validate_prepared_output(&output);
            assert!(
                findings.iter().any(|finding| {
                    finding.kind == OutputFindingKind::MissingImportedName
                        && finding.filename == "consumer.js"
                }),
                "an unproven callable surface must remain visible to validation: {findings:#?}"
            );

            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                consumer.contains("import { parse } from \"./provider.js\""),
                "an unproven property import must not be rewritten through the default export:\n{consumer}"
            );
        }
    }

    #[test]
    fn provider_facts_reject_conditional_properties_on_a_localized_callable_alias() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
function api(value) { return value; }
var local = module.exports = api;
if (globalThis.enableOptional) { local.optional = function() { return true; }; }
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var optional = require("./provider.js").optional;
module.exports = optional;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("conditional alias-property fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "a conditional alias property must remain visible to validation: {findings:#?}"
        );
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        assert!(
            consumer.contains("import { optional } from \"./provider.js\""),
            "an unproven alias property must not be repaired through the default export:\n{consumer}"
        );
    }

    #[test]
    fn provider_facts_do_not_infer_absent_callable_properties() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
function api(value) { return value; }
api.parse = function(value) { return value.length; };
module.exports = api;
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var missing = require("./provider.js").missing;
module.exports = function() { return missing; };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("absent callable-property fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "callable facts prove only observed properties and must not infer an absent one: {findings:#?}"
        );
    }

    #[test]
    fn provider_property_capture_preserves_same_declaration_order() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
module.exports = {
    capability: function() { return true; },
    secondary: function() { return true; }
};
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
let first = sideEffect(),
    { capability, secondary } = require("./provider.js"),
    available = Boolean(capability && secondary);
observe(first, available);
module.exports = available;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("same-declaration capture fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected consumer module");
        let effect = consumer
            .find("sideEffect()")
            .expect("leading declarator should remain");
        let capture = consumer
            .find(".capability")
            .or_else(|| consumer.find("{ capability"))
            .expect("property capture should remain");
        let secondary_capture = consumer[capture..]
            .find("secondary")
            .map(|offset| capture + offset)
            .expect("second property capture should remain");
        let feature_check = consumer
            .find("Boolean(")
            .expect("later declarator should remain");
        assert!(
            effect < capture && capture < secondary_capture && secondary_capture < feature_check,
            "captures must remain ordered between surrounding declarators:\n{consumer}"
        );
    }

    #[test]
    fn provider_facts_repair_recovered_commonjs_import_shapes() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "default-provider".to_string(),
                    code: r#"
var methods = {
    map: function(value) { return value + 1; },
    filter: function(value) { return value > 0; }
};
module.exports = methods;
"#
                    .to_string(),
                    filename: "default-provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "named-consumer".to_string(),
                    is_entry: true,
                    code: r#"
before();
var map = require("./default-provider.js").map;
between();
var filter = require("./default-provider.js").filter;
map = replacement;
module.exports = function(value) { return filter(map(value)); };
"#
                    .to_string(),
                    filename: "named-consumer.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("provider-aware fixture should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);

        let named_consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "named-consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected named consumer");
        assert!(
            !named_consumer.contains("import { map")
                && !named_consumer.contains("filter } from"),
            "properties of a proven default object must be captured from its default import:\n{named_consumer}"
        );
        assert!(
            (named_consumer.contains("{ map:") && named_consumer.contains("filter }"))
                || (named_consumer.contains(".map") && named_consumer.contains(".filter")),
            "property captures must come from the proven default object:\n{named_consumer}"
        );
        assert!(
            named_consumer.contains("let map") || named_consumer.contains("let { map"),
            "a reassigned property local must remain mutable instead of becoming an import binding:\n{named_consumer}"
        );
        let before = named_consumer
            .find("before();")
            .expect("leading effect should remain");
        let map_capture = named_consumer[before..]
            .find("map")
            .map(|offset| before + offset)
            .expect("map capture should remain");
        let between = named_consumer
            .find("between();")
            .expect("middle effect should remain");
        let filter_capture = named_consumer[between..]
            .find("filter")
            .map(|offset| between + offset)
            .expect("filter capture should remain");
        assert!(
            before < map_capture && map_capture < between && between < filter_capture,
            "property captures must stay at their original require positions around effects:\n{named_consumer}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("provider-aware fixture should decompile through the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "provider repair must survive the source-map path's second parse"
        );
    }

    #[test]
    fn commonjs_default_object_composition_preserves_mutable_identity_and_copy_order() {
        let modules = || {
            vec![
                UnpackedModule {
                    id: "base-a".to_string(),
                    code: r#"module.exports = { alpha: "a", shared: "first" };"#.to_string(),
                    filename: "base-a.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "base-b".to_string(),
                    code: r#"module.exports = { beta: "b", shared: "second" };"#.to_string(),
                    filename: "base-b.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "middle".to_string(),
                    code: r#"
module.exports = {};
Object.assign(module.exports, require("./base-a.js") || {});
"#
                    .to_string(),
                    filename: "middle.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "entry".to_string(),
                    is_entry: true,
                    code: r#"
module.exports = {};
Object.assign(module.exports, require("./middle.js") || {});
Object.assign(module.exports, require("./base-b.js") || {});
"#
                    .to_string(),
                    filename: "entry.js".to_string(),
                    ..Default::default()
                },
            ]
        };

        let output = unpack_multi_module(modules(), DecompileOptions::default())
            .expect("default-object composition should decompile");
        assert_eq!(validate_prepared_output(&output), vec![]);

        for filename in ["middle.js", "entry.js"] {
            let code = output
                .modules
                .iter()
                .find(|module| module.filename == filename)
                .map(|module| module.code.as_str())
                .expect("expected composed module");
            assert!(
                !code.contains("module.exports") && !code.contains("require("),
                "the exact composition should use recovered ESM edges:\n{code}"
            );
            assert!(
                !code.contains("export default {};"),
                "the exported value must be the same object that Object.assign mutates:\n{code}"
            );
            assert!(
                code.contains("Object.assign(_defaultObject,")
                    && code.contains("export default _defaultObject;"),
                "both the copies and default export must share one local object:\n{code}"
            );
        }

        let entry = output
            .modules
            .iter()
            .find(|module| module.filename == "entry.js")
            .map(|module| module.code.as_str())
            .expect("expected entry module");
        assert_eq!(
            entry.matches("Object.assign(").count(),
            2,
            "both ordered copies must remain visible:\n{entry}"
        );
        assert!(
            entry.find("./middle.js").expect("middle import")
                < entry.find("./base-b.js").expect("base-b import"),
            "provider imports should retain first-use order:\n{entry}"
        );

        let source_map_output = unpack_multi_module(
            modules(),
            DecompileOptions {
                emit_source_map: true,
                ..Default::default()
            },
        )
        .expect("composition recovery should survive the source-map path");
        assert_eq!(validate_prepared_output(&source_map_output), vec![]);
        assert!(
            source_map_output
                .modules
                .iter()
                .all(|module| module.source_map.is_some()),
            "composition recovery must preserve emitted source maps"
        );
    }

    #[test]
    fn commonjs_default_object_composition_requires_complete_provider_and_consumer_proofs() {
        let cases = [
            (
                "unknown-provider",
                "module.exports = makeStyles();",
                r#"
module.exports = {};
Object.assign(module.exports, require("./provider.js") || {});
"#,
            ),
            (
                "authored-esm-provider",
                "export default { value: 1 };",
                r#"
module.exports = {};
Object.assign(module.exports, require("./provider.js") || {});
"#,
            ),
            (
                "mutated-provider-surface",
                "module.exports = {}; module.exports.value = 1;",
                r#"
module.exports = {};
Object.assign(module.exports, require("./provider.js") || {});
"#,
            ),
            (
                "extra-consumer-runtime-use",
                "module.exports = { value: 1 };",
                r#"
module.exports = {};
observe(module.exports);
Object.assign(module.exports, require("./provider.js") || {});
"#,
            ),
        ];

        for (name, provider, consumer) in cases {
            let modules = vec![
                UnpackedModule {
                    id: "provider".to_string(),
                    code: provider.to_string(),
                    filename: "provider.js".to_string(),
                    ..Default::default()
                },
                UnpackedModule {
                    id: "consumer".to_string(),
                    is_entry: true,
                    code: consumer.to_string(),
                    filename: "consumer.js".to_string(),
                    ..Default::default()
                },
            ];
            let output = unpack_multi_module(modules, DecompileOptions::default())
                .unwrap_or_else(|error| panic!("{name} fixture should decompile: {error}"));
            let consumer = output
                .modules
                .iter()
                .find(|module| module.filename == "consumer.js")
                .map(|module| module.code.as_str())
                .expect("expected consumer module");
            assert!(
                consumer.contains("module.exports") && consumer.contains("require("),
                "{name} must fail closed instead of inventing a mutable default:\n{consumer}"
            );
            assert!(
                validate_prepared_output(&output)
                    .iter()
                    .any(|finding| finding.kind == OutputFindingKind::EsmCommonJsResidual),
                "{name} should retain an honest CommonJS residual"
            );
        }
    }

    #[test]
    fn commonjs_default_object_composition_cycles_fail_closed() {
        let modules = vec![
            UnpackedModule {
                id: "left".to_string(),
                is_entry: true,
                code: r#"
module.exports = {};
Object.assign(module.exports, require("./right.js") || {});
"#
                .to_string(),
                filename: "left.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "right".to_string(),
                code: r#"
module.exports = {};
Object.assign(module.exports, require("./left.js") || {});
"#
                .to_string(),
                filename: "right.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("cyclic composition fixture should decompile conservatively");
        assert!(output.modules.iter().all(|module| {
            module.code.contains("module.exports") && module.code.contains("require(")
        }));
        assert_eq!(
            validate_prepared_output(&output)
                .iter()
                .filter(|finding| finding.kind == OutputFindingKind::EsmCommonJsResidual)
                .count(),
            2,
            "the unresolved module target in each cyclic module should remain visible"
        );
    }

    #[test]
    fn provider_facts_leave_unproven_default_properties_unchanged() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "module.exports = makeProvider();".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var missing = require("./provider.js").missing;
module.exports = function() { return missing; };
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("unproven provider fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "an unknown default value must fail closed instead of fabricating a property capture: {findings:#?}"
        );
    }

    #[test]
    fn provider_facts_leave_mutated_commonjs_namespaces_unresolved() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "exports.value = 1;".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var provider = require("./provider.js");
provider.value = 2;
module.exports = provider;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("mutable namespace fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "a mutable CommonJS exports object needs a provider facade and must fail closed: {findings:#?}"
        );
    }

    #[test]
    fn provider_facts_leave_export_star_surfaces_unresolved() {
        let modules = vec![
            UnpackedModule {
                id: "star-source".to_string(),
                code: "exports.other = 1;".to_string(),
                filename: "star-source.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "provider".to_string(),
                code: r#"
module.exports = { value: 1 };
export * from "./star-source.js";
"#
                .to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"
var value = require("./provider.js").value;
module.exports = value;
"#
                .to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("export-star provider fixture should decompile");
        let findings = validate_prepared_output(&output);
        assert!(
            findings.iter().any(|finding| {
                finding.kind == OutputFindingKind::MissingImportedName
                    && finding.filename == "consumer.js"
            }),
            "an open export-star surface must fail closed instead of treating local absence as proof: {findings:#?}"
        );
    }

    #[test]
    fn provider_facts_do_not_rewrite_authored_esm_imports() {
        let modules = vec![
            UnpackedModule {
                id: "provider".to_string(),
                code: "export default { value: 1 };".to_string(),
                filename: "provider.js".to_string(),
                ..Default::default()
            },
            UnpackedModule {
                id: "consumer".to_string(),
                is_entry: true,
                code: r#"import { value } from "./provider.js"; console.log(value);"#.to_string(),
                filename: "consumer.js".to_string(),
                ..Default::default()
            },
        ];

        let output = unpack_multi_module(modules, DecompileOptions::default())
            .expect("authored ESM fixture should decompile");
        let consumer = output
            .modules
            .iter()
            .find(|module| module.filename == "consumer.js")
            .map(|module| module.code.as_str())
            .expect("expected ESM consumer");
        assert!(
            consumer.contains("import { value }"),
            "authored ESM imports must remain outside CommonJS repair:\n{consumer}"
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
        let code = &output.modules[0].code;

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
        let code = &output.modules[0].code;
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

        let source_map = output.modules[0]
            .source_map
            .as_deref()
            .expect("unpack should emit one source map per kept module");
        let sm = sourcemap::SourceMap::from_reader(source_map.as_bytes())
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
