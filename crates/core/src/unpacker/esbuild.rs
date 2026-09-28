use crate::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, Mark, SourceMap, Spanned};
use swc_core::ecma::ast::{Module, ModuleItem, Stmt};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{VisitMutWith, VisitWith};

use crate::unpacker::{
    module_item_declared_binding_ids, BindingId, BundleFormat, SourcePositions, UnpackResult,
    UnpackedModule,
};

mod bindings;
mod emit;
mod factories;
mod ownership;
mod scope;
mod scope_boundaries;
mod synthesis;

use bindings::{
    atom_binding_map_from_keys, build_item_binding_infos, collect_external_imports,
    collect_top_level_decl_indices, collect_top_level_decl_references,
    collect_top_level_decl_writes, collect_write_bindings, module_item_import_binding_ids,
    TopLevelRefCollector,
};
use emit::{
    emit_entry, emit_merged_module_plan, emit_standalone_group, group_standalone_factories,
    redirect_stub_modules,
};
use factories::{
    collect_commonjs_helper_syms, collect_factories, collect_factories_owned,
    collect_factory_analysis_bindings, collect_factory_body_bindings, collect_helper_syms,
    filter_helper_factory_declarators, has_factory_detection_evidence,
    item_has_helper_factory_declarator, locally_exported_atoms, Factory, FactoryBodyRef,
    PathCommentHints,
};
use ownership::{
    apply_demotion, claim_standalone_ownership, group_standalone_writers,
    partition_merged_factories, place_top_level_writers, plan_demotion, plan_merged_modules,
    FactoryOwnership, PendingFactory, StateAdoptionFilter, SupportClaimFilter, TopLevelIndex,
    TopLevelWriterItem,
};
use scope::{extract_scope_hoisted_modules, ScopeExtractionRefs, ScopeExtractionResult};
use scope_boundaries::{
    collect_scope_hoisted_boundaries, detect_export_helper, namespace_is_module_exported,
};
use synthesis::dedup_filename;

pub(super) fn detect_from_module_with_source(
    module: &Module,
    source: Option<&str>,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
    // Phase 1: cheap structural pre-checks on the unresolved module.
    // Both scans are O(top-level items) with no cloning or resolution.
    let helper_syms = {
        let span = tracing::info_span!("esbuild: collect helper syms");
        let _enter = span.enter();
        collect_helper_syms(module)
    };

    let has_export_helper_shape = detect_export_helper(&module.body).is_some();

    if helper_syms.is_empty() && !has_export_helper_shape {
        return None;
    }

    // Evidence of esbuild structure found — clone + resolve for binding analysis.
    let analysis_module = {
        let span = tracing::info_span!("esbuild: clone and resolve for analysis");
        let _enter = span.enter();
        let mut am = module.clone();
        am.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
        am
    };

    let commonjs_helper_syms = collect_commonjs_helper_syms(module);
    let filename_hints = source.map(|source| {
        let source_file = cm.lookup_source_file(module.span.lo);
        PathCommentHints::new(source, source_file.start_pos.0)
    });

    // Phase 2: collect factory declarations — `var X = helper(factory_fn)`.
    let factories = if helper_syms.is_empty() {
        vec![]
    } else {
        let span = tracing::info_span!("esbuild: collect factories");
        let _enter = span.enter();
        collect_factories(
            module,
            &analysis_module,
            &helper_syms,
            &commonjs_helper_syms,
            filename_hints.as_ref(),
        )
    };

    detect_from_prepared_factories(
        module,
        analysis_module,
        commonjs_helper_syms,
        factories,
        cm,
        positions,
    )
}

pub(super) fn detect_from_owned_factory_module_with_source(
    mut module: Module,
    source: Option<&str>,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Result<UnpackResult, Module> {
    let helper_syms = collect_helper_syms(&module);
    if helper_syms.is_empty() || !has_factory_detection_evidence(&module, &helper_syms) {
        return Err(module);
    }

    let analysis_module = {
        let span = tracing::info_span!("esbuild: clone and resolve owned candidate for analysis");
        let _enter = span.enter();
        let mut analysis = module.clone();
        analysis.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));
        analysis
    };
    let commonjs_helper_syms = collect_commonjs_helper_syms(&module);
    let filename_hints = source.map(|source| {
        let source_file = cm.lookup_source_file(module.span.lo);
        PathCommentHints::new(source, source_file.start_pos.0)
    });
    let factories = collect_factories_owned(
        &mut module,
        &analysis_module,
        &helper_syms,
        &commonjs_helper_syms,
        filename_hints.as_ref(),
    );

    Ok(detect_from_prepared_factories(
        &module,
        analysis_module,
        commonjs_helper_syms,
        factories,
        cm,
        positions,
    )
    .expect("owned factory evidence must produce an esbuild/Bun bundle"))
}

fn detect_from_prepared_factories(
    module: &Module,
    analysis_module: Module,
    commonjs_helper_syms: HashSet<Atom>,
    factories: Vec<Factory>,
    cm: Lrc<SourceMap>,
    positions: SourcePositions,
) -> Option<UnpackResult> {
    let helper_syms: HashSet<Atom> = factories
        .iter()
        .map(|factory| factory.helper_sym.clone())
        .collect();

    let has_cjs_factories = !commonjs_helper_syms.is_empty()
        && factories
            .iter()
            .any(|f| commonjs_helper_syms.contains(&f.helper_sym));
    let has_factories = has_cjs_factories || factories.len() >= 5;

    // Try scope-hoisted detection on the full module body (needed for
    // scope-only bundles that have no factories at all).
    let has_scope_hoisted = {
        let span = tracing::info_span!("esbuild: detect scope-hoisted");
        let _enter = span.enter();
        detect_export_helper(&analysis_module.body)
            .map(|(_, helper)| {
                let boundaries = collect_scope_hoisted_boundaries(&analysis_module.body, &helper);
                match boundaries.len() {
                    0 => false,
                    1 => {
                        let refs = build_item_binding_infos(&analysis_module.body);
                        namespace_is_module_exported(
                            &analysis_module.body,
                            &refs,
                            &boundaries[0].ns_binding,
                        )
                    }
                    _ => true,
                }
            })
            .unwrap_or(false)
    };

    if !has_factories && !has_scope_hoisted {
        return None;
    }

    let factory_syms: HashSet<Atom> = factories.iter().map(|f| f.var_name.clone()).collect();

    // Phase 3: assign filenames to factories (dedup), collect their referenced
    // bindings from the resolved AST.  Emission is deferred to Phase 6 so that
    // scope-hoisted extraction can inform import/export synthesis.
    let mut modules: Vec<UnpackedModule> = Vec::new();
    let mut global_seen: HashSet<String> = HashSet::default();
    global_seen.insert("entry.js".to_string());

    // Build top_level_bindings from the FULL analysis module so we can track
    // which identifiers referenced by factory bodies are top-level declarations
    // (potentially belonging to scope-hoisted modules).
    let all_top_level_bindings: HashSet<BindingId> = analysis_module
        .body
        .iter()
        .flat_map(|item| {
            module_item_declared_binding_ids(item)
                .into_iter()
                .chain(module_item_import_binding_ids(item))
        })
        .collect();
    // Keep only item indices here. One top-level declaration can own many
    // bindings, so retaining a cloned source + analysis item per binding
    // multiplies large Bun/esbuild ASTs. Clone the selected item only when a
    // recovered module actually needs to own it.
    let decl_indices = collect_top_level_decl_indices(&analysis_module.body);
    let helper_factory_syms: HashSet<Atom> = helper_syms
        .iter()
        .chain(factory_syms.iter())
        .cloned()
        .collect();
    let index = TopLevelIndex {
        decl_binding_by_atom: atom_binding_map_from_keys(&decl_indices),
        decl_references: collect_top_level_decl_references(
            &analysis_module.body,
            &decl_indices,
            &all_top_level_bindings,
            &helper_factory_syms,
        ),
        decl_writes: collect_top_level_decl_writes(
            &analysis_module.body,
            &decl_indices,
            &all_top_level_bindings,
        ),
        decl_indices,
        external_imports: collect_external_imports(&analysis_module.body, &module.body),
    };

    // Inventory every top-level statement that writes a top-level binding, at
    // any depth. A later ownership pass may move a written binding's
    // declaration into a standalone lazy factory; every such writer must
    // follow the same mutable binding — or the split must be cancelled.
    let top_level_writer_items: Vec<TopLevelWriterItem> = analysis_module
        .body
        .iter()
        .enumerate()
        .filter_map(|(source_index, item)| {
            let ModuleItem::Stmt(stmt) = item else {
                return None;
            };
            let mut write_targets = HashSet::default();
            collect_write_bindings(stmt, &all_top_level_bindings, &mut write_targets);
            if write_targets.is_empty() {
                return None;
            }
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &all_top_level_bindings,
                references: HashSet::default(),
            };
            item.visit_with(&mut collector);
            Some(TopLevelWriterItem {
                source_index,
                write_targets,
                referenced_bindings: collector.references,
                declared_bindings: module_item_declared_binding_ids(item).into_iter().collect(),
                relocatable_shape: !matches!(stmt, Stmt::Decl(_)),
                span: item.span(),
            })
        })
        .collect();

    let mut pending_factories: Vec<PendingFactory> = Vec::new();
    for factory in factories {
        let filename = dedup_filename(&factory.filename, &mut global_seen);

        // Read the resolved body from the analysis module by location instead
        // of retaining a second cloned body for every factory.
        let (referenced_bindings, write_bindings) = collect_factory_analysis_bindings(
            &analysis_module,
            factory.analysis_location,
            &all_top_level_bindings,
        )
        .unwrap_or_else(|| {
            collect_factory_body_bindings(
                FactoryBodyRef::Stmts(&factory.body_stmts),
                &all_top_level_bindings,
            )
        });

        pending_factories.push(PendingFactory {
            binding: factory.binding,
            var_name: factory.var_name,
            filename,
            cjs_params: factory.cjs_params,
            body_stmts: factory.body_stmts,
            referenced_bindings,
            write_bindings,
            span: factory.span,
        });
    }

    // Aggregate all factory-referenced bindings for scope-hoisted export expansion.
    let all_factory_referenced: HashSet<BindingId> = pending_factories
        .iter()
        .flat_map(|f| f.referenced_bindings.iter().cloned())
        .collect();
    // Every factory preassigns its own binding and the top-level state its
    // body writes. A CommonJS factory follows the same path as a lazy ESM
    // initializer: a scope module that writes that state claims the factory,
    // and the merged emission keeps the factory's cached callable. Excluding
    // CommonJS factories here left their callers without an import edge.
    let mut factory_preassigned_bindings: HashMap<BindingId, String> = HashMap::default();
    for factory in &pending_factories {
        factory_preassigned_bindings.insert(factory.binding.clone(), factory.filename.clone());
        for write_binding in &factory.write_bindings {
            factory_preassigned_bindings.insert(write_binding.clone(), factory.filename.clone());
        }
    }
    let support_claim_filter = SupportClaimFilter {
        helper_syms: &helper_syms,
        factory_syms: &factory_syms,
        top_level_decl_indices: &index.decl_indices,
    };
    let mut factory_importable_bindings = factory_preassigned_bindings.clone();
    for factory in &pending_factories {
        for ref_binding in &factory.referenced_bindings {
            if factory.write_bindings.contains(ref_binding)
                || factory_importable_bindings.contains_key(ref_binding)
                || !support_claim_filter.admits(ref_binding)
            {
                continue;
            }
            factory_importable_bindings.insert(ref_binding.clone(), factory.filename.clone());
        }
    }

    // Phase 4: everything that is not a helper decl or factory decl becomes the entry.
    // Mixed declarations can contain useful sibling helpers, for example
    // `var wrap = ..., __esm = ...`; filter at declarator granularity.
    // A code-splitting chunk can export its runtime helpers to other chunks
    // (`export { __commonJS as a }`); those declarations stay in the entry so
    // the export keeps a binding.
    let exported_helper_syms = locally_exported_atoms(&module.body);
    let entry_dropped_syms: HashSet<Atom> = helper_factory_syms
        .iter()
        .filter(|sym| factory_syms.contains(*sym) || !exported_helper_syms.contains(*sym))
        .cloned()
        .collect();
    let mut drop_unowned_helper_sibling_indices = HashSet::default();
    let mut entry_items = Vec::new();
    let mut analysis_entry_items = Vec::new();
    for (source_item, analysis_item) in module.body.iter().zip(&analysis_module.body) {
        let source_filtered = filter_helper_factory_declarators(source_item, &entry_dropped_syms);
        let analysis_filtered =
            filter_helper_factory_declarators(analysis_item, &entry_dropped_syms);
        if source_filtered.is_some() != analysis_filtered.is_some() {
            continue;
        }
        let Some(source_filtered) = source_filtered else {
            continue;
        };
        let Some(analysis_filtered) = analysis_filtered else {
            continue;
        };
        if item_has_helper_factory_declarator(analysis_item, &entry_dropped_syms) {
            drop_unowned_helper_sibling_indices.insert(entry_items.len());
        }
        entry_items.push(source_filtered);
        analysis_entry_items.push(analysis_filtered);
    }
    drop(analysis_module);

    // Phase 5: split scope-hoisted modules out of the entry items.
    // Pass factory-referenced bindings so the extraction can expand exports
    // and return binding→module mapping for factory import synthesis.
    let ScopeExtractionResult {
        modules: scope_hoisted_modules,
        remaining_entry,
        binding_to_filename,
        module_already_imports,
        module_local_atoms,
        module_referenced_atoms,
        scope_claimed_factory_bindings,
    } = {
        let span = tracing::info_span!("esbuild: extract scope-hoisted modules");
        let _enter = span.enter();
        extract_scope_hoisted_modules(
            &analysis_entry_items,
            entry_items,
            &mut global_seen,
            cm.clone(),
            ScopeExtractionRefs {
                factory_referenced: &all_factory_referenced,
                factory_preassigned_bindings: &factory_preassigned_bindings,
                factory_importable_bindings: &factory_importable_bindings,
                drop_unowned_helper_sibling_indices: &drop_unowned_helper_sibling_indices,
                positions,
            },
        )
    };
    modules.extend(scope_hoisted_modules);

    // Phase 6: decide where every factory, support declaration, and state
    // writer goes, then emit. Emission never changes ownership.
    let mut ownership = FactoryOwnership::new(binding_to_filename);
    let remaining_entry_spans: HashSet<(u32, u32)> = remaining_entry
        .iter()
        .map(|item| (item.span().lo.0, item.span().hi.0))
        .collect();
    let entry_written_state: HashSet<BindingId> = top_level_writer_items
        .iter()
        .filter(|writer| remaining_entry_spans.contains(&(writer.span.lo.0, writer.span.hi.0)))
        .flat_map(|writer| {
            writer
                .write_targets
                .iter()
                .filter(|target| !writer.declared_bindings.contains(*target))
                .cloned()
        })
        .collect();
    let (merged_factories, mut standalone_factories) = partition_merged_factories(
        pending_factories,
        &scope_claimed_factory_bindings,
        &factory_importable_bindings,
        &StateAdoptionFilter {
            support: &support_claim_filter,
            entry_written: &entry_written_state,
        },
        &index,
        &mut ownership,
    );
    claim_standalone_ownership(
        &standalone_factories,
        &index.decl_references,
        &support_claim_filter,
        &mut ownership.binding_to_filename,
        &mut ownership.factory_owned_bindings,
    );
    group_standalone_writers(&mut standalone_factories, &index, &mut ownership);
    ownership.debug_assert_consistent("grouping");

    let requested_demotions = place_top_level_writers(
        top_level_writer_items,
        &module.body,
        &remaining_entry_spans,
        &standalone_factories,
        &index,
        &mut ownership,
    );
    let mut demoted_factories = Vec::new();
    if !requested_demotions.is_empty() {
        if let Some(demoted) = plan_demotion(
            requested_demotions,
            &standalone_factories,
            &merged_factories,
            &module_referenced_atoms,
            &remaining_entry_spans,
            &index,
            &ownership,
        ) {
            demoted_factories = apply_demotion(&demoted, &mut standalone_factories, &mut ownership);
        }
    }
    let merged_module_plans = plan_merged_modules(
        &modules,
        merged_factories,
        &index,
        &module_already_imports,
        &module_local_atoms,
        &module_referenced_atoms,
        &mut ownership,
    );
    ownership.debug_assert_consistent("planning");

    for plan in merged_module_plans {
        emit_merged_module_plan(
            &mut modules[plan.module_index],
            plan,
            &module.body,
            &index.external_imports,
            cm.clone(),
            positions,
        );
    }
    let standalone_factory_write_bindings: HashSet<BindingId> = standalone_factories
        .iter()
        .flat_map(|factory| factory.write_bindings.iter().cloned())
        .collect();
    for (group_filename, factories) in group_standalone_factories(standalone_factories) {
        modules.push(emit_standalone_group(
            group_filename,
            factories,
            &module.body,
            &index,
            &ownership,
            cm.clone(),
            positions,
        ));
    }
    modules.extend(redirect_stub_modules(&ownership.redirects));
    if !remaining_entry.is_empty() || !demoted_factories.is_empty() {
        modules.push(emit_entry(
            remaining_entry,
            demoted_factories,
            &standalone_factory_write_bindings,
            &index,
            &ownership,
            cm,
            positions,
        ));
    }

    Some(UnpackResult::without_cycle_warnings(
        modules,
        BundleFormat::Esbuild,
    ))
}

#[cfg(test)]
mod tests;
