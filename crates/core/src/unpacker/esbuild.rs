use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{
    sync::Lrc, FileName, Mark, SourceMap, Span, Spanned, SyntaxContext, DUMMY_SP, GLOBALS,
};
use swc_core::ecma::ast::{
    ArrowExpr, AssignTarget, AssignTargetPat, BindingIdent, BlockStmt, BlockStmtOrExpr, Bool,
    CallExpr, Callee, ClassDecl, Decl, ExportDecl, ExportNamedSpecifier, ExportSpecifier, Expr,
    ExprOrSpread, ExprStmt, FnDecl, ForInStmt, Function, Ident, IdentName, ImportDecl,
    ImportNamedSpecifier, ImportSpecifier, KeyValueProp, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectLit, ObjectPatProp, Pat, Prop,
    PropName, PropOrSpread, SimpleAssignTarget, Stmt, Str, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::module_path::relative_import_specifier;
use crate::rules::rename_utils::{rename_bindings, BindingRename};
use crate::unpacker::{
    module_item_declared_binding_ids, span_byte_range, spans_byte_ranges, BindingId, BundleFormat,
    UnpackResult, UnpackedModule,
};

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "esbuild.js", cm.clone()).ok()?;
        detect_from_module_with_source(&module, Some(source), cm)
    })
}

pub(super) fn detect_from_module_with_source(
    module: &Module,
    source: Option<&str>,
    cm: Lrc<SourceMap>,
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
    let mut global_seen: HashSet<String> = HashSet::new();
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
    let external_imports = collect_external_imports(&analysis_module.body, &module.body);
    let mut top_level_decl_items: HashMap<BindingId, (usize, ModuleItem, ModuleItem)> =
        HashMap::new();
    for (index, (analysis_item, source_item)) in analysis_module
        .body
        .iter()
        .zip(module.body.iter())
        .enumerate()
    {
        for binding in module_item_declared_binding_ids(analysis_item) {
            top_level_decl_items
                .entry(binding)
                .or_insert_with(|| (index, source_item.clone(), analysis_item.clone()));
        }
    }
    let top_level_decl_binding_by_atom = atom_binding_map_from_keys(&top_level_decl_items);

    struct PendingFactory {
        binding: BindingId,
        var_name: Atom,
        filename: String,
        cjs_params: Option<CjsFactoryParams>,
        body_stmts: Vec<Stmt>,
        referenced_bindings: HashSet<BindingId>,
        write_bindings: HashSet<BindingId>,
        span: Span,
    }

    let mut pending_factories: Vec<PendingFactory> = Vec::new();
    for factory in factories {
        let filename = dedup_filename(&factory.filename, &mut global_seen);

        // Collect which top-level bindings this factory's body references
        // by visiting the resolved (analysis) body stmts.
        let mut referenced_bindings = HashSet::new();
        let mut write_bindings = HashSet::new();
        for stmt in &factory.analysis_body_stmts {
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &all_top_level_bindings,
                references: HashSet::new(),
            };
            stmt.visit_with(&mut collector);
            referenced_bindings.extend(collector.references.clone());
            collect_write_bindings(stmt, &all_top_level_bindings, &mut write_bindings);
        }

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
    let mut factory_preassigned_bindings: HashMap<BindingId, String> = HashMap::new();
    for factory in &pending_factories {
        if factory.cjs_params.is_some() {
            continue;
        }
        factory_preassigned_bindings.insert(factory.binding.clone(), factory.filename.clone());
        for write_binding in &factory.write_bindings {
            factory_preassigned_bindings.insert(write_binding.clone(), factory.filename.clone());
        }
    }
    let mut factory_importable_bindings = factory_preassigned_bindings.clone();
    factory_importable_bindings.extend(
        pending_factories
            .iter()
            .filter(|factory| factory.cjs_params.is_none() || factory.write_bindings.is_empty())
            .map(|factory| (factory.binding.clone(), factory.filename.clone())),
    );
    for factory in &pending_factories {
        for ref_binding in &factory.referenced_bindings {
            if factory.write_bindings.contains(ref_binding)
                || factory_importable_bindings.contains_key(ref_binding)
                || helper_syms.contains(&ref_binding.0)
                || factory_syms.contains(&ref_binding.0)
                || !top_level_decl_items.contains_key(ref_binding)
            {
                continue;
            }
            factory_importable_bindings.insert(ref_binding.clone(), factory.filename.clone());
        }
    }

    // Phase 4: everything that is not a helper decl or factory decl becomes the entry.
    // Mixed declarations can contain useful sibling helpers, for example
    // `var wrap = ..., __esm = ...`; filter at declarator granularity.
    let helper_factory_syms: HashSet<Atom> = helper_syms
        .iter()
        .chain(factory_syms.iter())
        .cloned()
        .collect();
    let mut drop_unowned_helper_sibling_indices = HashSet::new();
    let mut entry_items = Vec::new();
    let mut analysis_entry_items = Vec::new();
    for (source_item, analysis_item) in module.body.iter().zip(&analysis_module.body) {
        let source_filtered = filter_helper_factory_declarators(source_item, &helper_factory_syms);
        let analysis_filtered =
            filter_helper_factory_declarators(analysis_item, &helper_factory_syms);
        if source_filtered.is_some() != analysis_filtered.is_some() {
            continue;
        }
        let Some(source_filtered) = source_filtered else {
            continue;
        };
        let Some(analysis_filtered) = analysis_filtered else {
            continue;
        };
        if item_has_helper_factory_declarator(analysis_item, &helper_factory_syms) {
            drop_unowned_helper_sibling_indices.insert(entry_items.len());
        }
        entry_items.push(source_filtered);
        analysis_entry_items.push(analysis_filtered);
    }

    // Phase 5: split scope-hoisted modules out of the entry items.
    // Pass factory-referenced bindings so the extraction can expand exports
    // and return binding→module mapping for factory import synthesis.
    let (
        scope_hoisted_modules,
        remaining_entry,
        mut binding_to_filename,
        module_already_imports,
        module_local_atoms,
        module_referenced_atoms,
        scope_claimed_factory_bindings,
    ) = {
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
            },
        )
    };
    modules.extend(scope_hoisted_modules);

    // Phase 6: emit each factory module, now with synthesized imports for any
    // references to scope-hoisted module bindings.
    //
    // Init-factory merging: if a factory writes to bindings that ALL belong to
    // a single scope-hoisted module, it's an init function for that module.
    // Merge its body into the target module rather than emitting a separate file
    // with invalid ESM (imports are read-only, so `import {x} ...; x = ...`
    // would be a runtime error).
    struct MergedFactory {
        var_name: Atom,
        cjs_params: Option<CjsFactoryParams>,
        stmts: Vec<Stmt>,
        referenced_bindings: HashSet<BindingId>,
        write_bindings: HashSet<BindingId>,
    }

    let mut merged_factories: HashMap<String, Vec<MergedFactory>> = HashMap::new();
    let mut standalone_factories: Vec<PendingFactory> = Vec::new();
    let mut factory_owned_bindings: HashMap<String, HashSet<BindingId>> = HashMap::new();

    for factory in pending_factories {
        if factory.write_bindings.is_empty() {
            standalone_factories.push(factory);
            continue;
        }

        // Check if all write targets belong to the same scope-hoisted module.
        let mut target_filename: Option<String> = None;
        let mut is_single_target = true;
        for wb in &factory.write_bindings {
            if let Some(fname) = binding_to_filename.get(wb) {
                match &target_filename {
                    None => target_filename = Some(fname.clone()),
                    Some(existing) if existing == fname => {}
                    Some(_) => {
                        is_single_target = false;
                        break;
                    }
                }
            } else {
                is_single_target = false;
                break;
            }
        }

        let is_scope_claimed_init = factory
            .write_bindings
            .iter()
            .any(|binding| scope_claimed_factory_bindings.contains_key(binding));
        let can_merge = factory.cjs_params.is_some() || is_scope_claimed_init;

        if let (true, Some(fname), true) = (is_single_target, target_filename, can_merge) {
            binding_to_filename.insert(factory.binding.clone(), fname.clone());
            for write_binding in &factory.write_bindings {
                let owned_binding = top_level_decl_binding_by_atom
                    .get(&write_binding.0)
                    .unwrap_or(write_binding);
                if scope_claimed_factory_bindings.contains_key(write_binding)
                    || scope_claimed_factory_bindings.contains_key(owned_binding)
                {
                    binding_to_filename.insert(owned_binding.clone(), fname.clone());
                    factory_owned_bindings
                        .entry(fname.clone())
                        .or_default()
                        .insert(owned_binding.clone());
                }
            }
            merged_factories
                .entry(fname)
                .or_default()
                .push(MergedFactory {
                    var_name: factory.var_name,
                    cjs_params: factory.cjs_params,
                    stmts: factory.body_stmts,
                    referenced_bindings: factory.referenced_bindings,
                    write_bindings: factory.write_bindings,
                });
        } else {
            standalone_factories.push(factory);
        }
    }

    for factory in &standalone_factories {
        binding_to_filename
            .entry(factory.binding.clone())
            .or_insert_with(|| factory.filename.clone());
        for write_binding in &factory.write_bindings {
            binding_to_filename
                .entry(write_binding.clone())
                .or_insert_with(|| factory.filename.clone());
            factory_owned_bindings
                .entry(factory.filename.clone())
                .or_default()
                .insert(write_binding.clone());
        }
    }
    for factory in &standalone_factories {
        for ref_binding in &factory.referenced_bindings {
            if factory.write_bindings.contains(ref_binding)
                || binding_to_filename.contains_key(ref_binding)
                || helper_syms.contains(&ref_binding.0)
                || factory_syms.contains(&ref_binding.0)
                || !top_level_decl_items.contains_key(ref_binding)
            {
                continue;
            }
            binding_to_filename.insert(ref_binding.clone(), factory.filename.clone());
            factory_owned_bindings
                .entry(factory.filename.clone())
                .or_default()
                .insert(ref_binding.clone());
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for factory in &standalone_factories {
            let owned_analysis_decl_items = factory_owned_analysis_decl_items(
                &factory.filename,
                &factory_owned_bindings,
                &top_level_decl_items,
            );
            for item in &owned_analysis_decl_items {
                let mut collector = TopLevelRefCollector {
                    top_level_bindings: &all_top_level_bindings,
                    references: HashSet::new(),
                };
                item.visit_with(&mut collector);
                for ref_binding in collector.references {
                    if binding_to_filename.contains_key(&ref_binding)
                        || helper_syms.contains(&ref_binding.0)
                        || factory_syms.contains(&ref_binding.0)
                        || !top_level_decl_items.contains_key(&ref_binding)
                    {
                        continue;
                    }
                    binding_to_filename.insert(ref_binding.clone(), factory.filename.clone());
                    factory_owned_bindings
                        .entry(factory.filename.clone())
                        .or_default()
                        .insert(ref_binding);
                    changed = true;
                }
            }
        }
    }

    let binding_filename_by_atom = atom_to_filename_binding_map(&binding_to_filename);
    let external_import_by_atom = atom_binding_map_from_keys(&external_imports);

    // Append merged factory bodies to their target modules, synthesizing
    // imports for any cross-module reads the factory body needs.
    if !merged_factories.is_empty() {
        for module in &mut modules {
            let Some(factories) = merged_factories.remove(&module.filename) else {
                continue;
            };
            let mut extra_imports: HashMap<String, Vec<Atom>> = HashMap::new();
            let mut extra_external_imports: HashSet<BindingId> = HashSet::new();
            let mut extra_owned_bindings: HashSet<BindingId> = HashSet::new();
            let mut merged_init_bodies: Vec<(Atom, Vec<Stmt>)> = Vec::new();

            let already_imported = module_already_imports
                .get(&module.filename)
                .cloned()
                .unwrap_or_default();

            for mf in factories {
                if mf.cjs_params.is_some() {
                    continue;
                }
                for write_binding in &mf.write_bindings {
                    let owned_binding = top_level_decl_binding_by_atom
                        .get(&write_binding.0)
                        .unwrap_or(write_binding);
                    if binding_to_filename
                        .get(owned_binding)
                        .is_some_and(|filename| filename == &module.filename)
                        && top_level_decl_items.contains_key(owned_binding)
                    {
                        extra_owned_bindings.insert(owned_binding.clone());
                    }
                }
                for ref_binding in &mf.referenced_bindings {
                    if mf.write_bindings.contains(ref_binding) {
                        continue;
                    }
                    if already_imported.contains(ref_binding) {
                        continue;
                    }
                    if let Some(source_filename) = binding_to_filename.get(ref_binding) {
                        if *source_filename != module.filename {
                            extra_imports
                                .entry(source_filename.clone())
                                .or_default()
                                .push(ref_binding.0.clone());
                        }
                    } else if let Some((source_binding, source_filename)) =
                        binding_filename_by_atom.get(&ref_binding.0)
                    {
                        if *source_filename != module.filename {
                            extra_imports
                                .entry(source_filename.clone())
                                .or_default()
                                .push(source_binding.0.clone());
                        }
                    } else if external_imports.contains_key(ref_binding) {
                        extra_external_imports.insert(ref_binding.clone());
                    } else if let Some(import_binding) = external_import_by_atom.get(&ref_binding.0)
                    {
                        extra_external_imports.insert(import_binding.clone());
                    } else if top_level_decl_items.contains_key(ref_binding) {
                        extra_owned_bindings.insert(ref_binding.clone());
                    }
                }
                merged_init_bodies.push((mf.var_name, mf.stmts));
            }

            let mut changed = true;
            while changed {
                changed = false;
                let owned_bindings: Vec<BindingId> = extra_owned_bindings.iter().cloned().collect();
                for owned_binding in owned_bindings {
                    let Some((_, _, analysis_item)) = top_level_decl_items.get(&owned_binding)
                    else {
                        continue;
                    };
                    let mut collector = TopLevelRefCollector {
                        top_level_bindings: &all_top_level_bindings,
                        references: HashSet::new(),
                    };
                    analysis_item.visit_with(&mut collector);
                    for ref_binding in collector.references {
                        if extra_owned_bindings.contains(&ref_binding)
                            || already_imported.contains(&ref_binding)
                        {
                            continue;
                        }
                        if let Some(source_filename) = binding_to_filename.get(&ref_binding) {
                            if *source_filename != module.filename {
                                extra_imports
                                    .entry(source_filename.clone())
                                    .or_default()
                                    .push(ref_binding.0.clone());
                            }
                        } else if let Some((source_binding, source_filename)) =
                            binding_filename_by_atom.get(&ref_binding.0)
                        {
                            if *source_filename != module.filename {
                                extra_imports
                                    .entry(source_filename.clone())
                                    .or_default()
                                    .push(source_binding.0.clone());
                            }
                        } else if external_imports.contains_key(&ref_binding) {
                            extra_external_imports.insert(ref_binding);
                        } else if let Some(import_binding) =
                            external_import_by_atom.get(&ref_binding.0)
                        {
                            extra_external_imports.insert(import_binding.clone());
                        } else if top_level_decl_items.contains_key(&ref_binding) {
                            extra_owned_bindings.insert(ref_binding);
                            changed = true;
                        }
                    }
                }
            }

            let mut import_items: Vec<ModuleItem> = Vec::new();
            let mut merged_local_atoms = module_local_atoms
                .get(&module.filename)
                .cloned()
                .unwrap_or_default();
            merged_local_atoms.extend(
                binding_to_filename
                    .iter()
                    .filter(|(_, filename)| *filename == &module.filename)
                    .map(|((atom, _), _)| atom.clone()),
            );
            merged_local_atoms.extend(extra_owned_bindings.iter().map(|(atom, _)| atom.clone()));
            let mut external_imports_sorted: Vec<BindingId> =
                extra_external_imports.into_iter().collect();
            external_imports_sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for binding in external_imports_sorted {
                if merged_local_atoms.contains(&binding.0) {
                    continue;
                }
                if let Some(import) = external_imports.get(&binding) {
                    import_items.push(make_external_import_stmt(import));
                }
            }
            let mut source_filenames: Vec<String> = extra_imports.keys().cloned().collect();
            source_filenames.sort();
            if let Some(referenced_atoms) = module_referenced_atoms.get(&module.filename) {
                augment_imports_with_referenced_atoms_for_existing_sources(
                    &mut extra_imports,
                    &module.filename,
                    referenced_atoms,
                    &binding_filename_by_atom,
                    Some(&merged_local_atoms),
                );
                source_filenames = extra_imports.keys().cloned().collect();
                source_filenames.sort();
            }
            for source_filename in source_filenames {
                let names = extra_imports.get_mut(&source_filename).unwrap();
                names.retain(|name| !merged_local_atoms.contains(name));
                names.sort();
                names.dedup();
                if names.is_empty() {
                    continue;
                }
                let rel_path = relative_import_path(&module.filename, &source_filename);
                import_items.push(make_scope_import_stmt(names, &rel_path));
            }

            let module_factory_owned = factory_owned_bindings
                .get(&module.filename)
                .cloned()
                .unwrap_or_default();
            let mut extra_owned_items: Vec<(usize, ModuleItem)> = extra_owned_bindings
                .into_iter()
                .filter(|binding| {
                    module_factory_owned.contains(binding)
                        || module_local_atoms
                            .get(&module.filename)
                            .is_none_or(|local_atoms| !local_atoms.contains(&binding.0))
                })
                .filter_map(|binding| {
                    top_level_decl_items
                        .get(&binding)
                        .map(|(index, source_item, _)| (*index, source_item.clone()))
                })
                .collect();
            extra_owned_items.sort_by_key(|(index, _)| *index);
            extra_owned_items.dedup_by_key(|(index, _)| *index);
            let body_items: Vec<ModuleItem> = import_items
                .into_iter()
                .chain(extra_owned_items.into_iter().map(|(_, item)| item))
                .chain(factory_owned_export_items(
                    &module.filename,
                    &factory_owned_bindings,
                ))
                .collect();
            let extra_code = emit_items(body_items, module.filename.clone(), cm.clone());
            module.code.push('\n');
            module.code.push_str(&extra_code);
            for (name, stmts) in merged_init_bodies {
                module.code.push_str(&emit_esm_init_function_code(
                    &name,
                    stmts,
                    module.filename.clone(),
                    cm.clone(),
                ));
            }
        }
    }

    for factory in standalone_factories {
        let owned_decl_items = factory_owned_decl_items(
            &factory.filename,
            &factory_owned_bindings,
            &top_level_decl_items,
        );
        let owned_analysis_decl_items = factory_owned_analysis_decl_items(
            &factory.filename,
            &factory_owned_bindings,
            &top_level_decl_items,
        );
        let declared_owned_atoms: HashSet<Atom> = owned_decl_items
            .iter()
            .flat_map(module_item_declared_binding_ids)
            .map(|(atom, _)| atom)
            .collect();
        let owned_export_atoms: HashSet<Atom> = factory_owned_bindings
            .get(&factory.filename)
            .into_iter()
            .flat_map(|bindings| bindings.iter())
            .map(|(atom, _)| atom.clone())
            .collect();
        let mut extended_referenced_bindings = factory.referenced_bindings.clone();
        for item in &owned_analysis_decl_items {
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &all_top_level_bindings,
                references: HashSet::new(),
            };
            item.visit_with(&mut collector);
            extended_referenced_bindings.extend(collector.references);
        }

        let mut import_items: Vec<ModuleItem> = Vec::new();
        let mut external_import_bindings: HashSet<BindingId> = HashSet::new();
        let mut import_renames: Vec<BindingRename> = Vec::new();

        if !binding_to_filename.is_empty() {
            // Group factory's referenced bindings by source module filename.
            let mut imports_by_source: HashMap<String, Vec<BindingId>> = HashMap::new();
            let owned = factory_owned_bindings
                .get(&factory.filename)
                .cloned()
                .unwrap_or_default();
            for ref_binding in &extended_referenced_bindings {
                // Don't import bindings that this factory writes to.
                if factory.write_bindings.contains(ref_binding)
                    || owned.contains(ref_binding)
                    || declared_owned_atoms.contains(&ref_binding.0)
                {
                    continue;
                }
                if let Some(source_filename) = binding_to_filename.get(ref_binding) {
                    if source_filename == &factory.filename {
                        continue;
                    }
                    imports_by_source
                        .entry(source_filename.clone())
                        .or_default()
                        .push(ref_binding.clone());
                } else if external_imports.contains_key(ref_binding) {
                    external_import_bindings.insert(ref_binding.clone());
                }
            }
            let mut reserved_import_atoms = declared_owned_atoms.clone();
            reserved_import_atoms.extend(owned_export_atoms.iter().cloned());
            reserved_import_atoms
                .extend(factory.write_bindings.iter().map(|(atom, _)| atom.clone()));
            let mut source_filenames: Vec<String> = imports_by_source.keys().cloned().collect();
            source_filenames.sort();
            for source_filename in source_filenames {
                let bindings = imports_by_source.get_mut(&source_filename).unwrap();
                bindings.sort_by(|a, b| a.0.cmp(&b.0));
                bindings.dedup();
                let mut names = Vec::new();
                for binding in bindings {
                    let imported = binding.0.clone();
                    let local = reserve_import_atom(&imported, &mut reserved_import_atoms);
                    if local != imported {
                        import_renames.push(BindingRename {
                            old: binding.clone(),
                            new: local.clone(),
                        });
                    }
                    names.push((imported, local));
                }
                let rel_path = relative_import_path(&factory.filename, &source_filename);
                import_items.push(make_scope_import_stmt_with_aliases(&names, &rel_path));
            }
        }
        let mut external_import_bindings: Vec<BindingId> =
            external_import_bindings.into_iter().collect();
        external_import_bindings.sort_by(|a, b| a.0.cmp(&b.0));
        for binding in external_import_bindings {
            if let Some(import) = external_imports.get(&binding) {
                import_items.push(make_external_import_stmt(import));
            }
        }

        let mut body_items: Vec<ModuleItem> = import_items
            .into_iter()
            .chain(owned_decl_items.clone())
            .chain(factory_owned_export_items(
                &factory.filename,
                &factory_owned_bindings,
            ))
            .collect();
        rename_bindings(&mut body_items, &import_renames);
        let mut factory_body_stmts = factory.body_stmts;
        rename_bindings(&mut factory_body_stmts, &import_renames);

        let mut write_names: Vec<Atom> = factory
            .write_bindings
            .iter()
            .filter(|binding| {
                binding_to_filename
                    .get(*binding)
                    .is_some_and(|filename| filename == &factory.filename)
                    && !declared_owned_atoms.contains(&binding.0)
            })
            .map(|(atom, _)| atom.clone())
            .collect();
        write_names.sort();
        write_names.dedup();

        let mut code = String::new();
        // Other modules may import and call this synthetic init function while
        // this module is still evaluating through an ESM cycle. Keep the
        // module-local storage it mutates before the exported callable wrapper,
        // otherwise later VarDeclToLetConst can turn trailing `var` storage
        // into TDZ-sensitive `let` declarations.
        if !body_items.is_empty() {
            code.push_str(&emit_items(
                body_items,
                factory.filename.clone(),
                cm.clone(),
            ));
        }
        if !write_names.is_empty() {
            let names = write_names
                .iter()
                .map(|name| name.as_ref())
                .collect::<Vec<_>>()
                .join(", ");
            code.push_str(&format!("export var {names};\n"));
        }
        if let Some(cjs_params) = &factory.cjs_params {
            let cache_name = format!("__wakaru_{}_cache", factory.var_name);
            code.push_str(&format!("var {cache_name};\n"));
            code.push_str(&format!("export function {}() {{\n", factory.var_name));
            let cached_return = cjs_params
                .module
                .as_ref()
                .map(|_| format!("{cache_name}.exports"))
                .unwrap_or_else(|| cache_name.clone());
            code.push_str(&format!("if ({cache_name}) return {cached_return};\n"));
            code.push_str(&format!("var {} = {{}};\n", cjs_params.exports));
            if let Some(module_name) = &cjs_params.module {
                code.push_str(&format!(
                    "var {module_name} = {{ exports: {} }};\n",
                    cjs_params.exports
                ));
                code.push_str(&format!("{cache_name} = {module_name};\n"));
            } else {
                code.push_str(&format!("{cache_name} = {};\n", cjs_params.exports));
            }
            code.push_str(&emit_items(
                factory_body_stmts
                    .into_iter()
                    .map(ModuleItem::Stmt)
                    .collect(),
                factory.filename.clone(),
                cm.clone(),
            ));
            let return_expr = cjs_params
                .module
                .as_ref()
                .map(|module_name| format!("{module_name}.exports"))
                .unwrap_or_else(|| cjs_params.exports.to_string());
            code.push_str(&format!("\nreturn {return_expr};\n}}\n"));
        } else {
            code.push_str(&emit_esm_init_function_code(
                &factory.var_name,
                factory_body_stmts,
                factory.filename.clone(),
                cm.clone(),
            ));
        }
        modules.push(UnpackedModule {
            id: factory.var_name.to_string(),
            is_entry: false,
            code,
            filename: factory.filename,
            source_ranges: span_byte_range(&cm, factory.span).into_iter().collect(),
            source_input: String::new(),
        });
    }

    if !remaining_entry.is_empty() {
        let entry_ranges = spans_byte_ranges(&cm, remaining_entry.iter().map(|item| item.span()));
        let remaining_entry = repair_entry_imports(remaining_entry, &binding_to_filename);
        let entry_module = Module {
            span: Default::default(),
            body: remaining_entry,
            shebang: None,
        };
        let code = emit_module(entry_module, "entry.js".to_string(), cm);
        modules.push(UnpackedModule {
            id: "entry".to_string(),
            is_entry: true,
            code,
            filename: "entry.js".to_string(),
            source_ranges: entry_ranges,
            source_input: String::new(),
        });
    }

    Some(UnpackResult::without_cycle_premerge(
        modules,
        BundleFormat::Esbuild,
    ))
}

fn emit_esm_init_function_code(
    name: &Atom,
    stmts: Vec<Stmt>,
    filename: String,
    cm: Lrc<SourceMap>,
) -> String {
    let guard = format!("__wakaru_{name}_initialized");
    let body = emit_items(
        stmts.into_iter().map(ModuleItem::Stmt).collect(),
        filename,
        cm,
    );
    format!("var {guard} = false;\nexport function {name}() {{\nif ({guard}) return;\n{guard} = true;\n{body}\n}}\n")
}

fn repair_entry_imports(
    entry_items: Vec<ModuleItem>,
    binding_to_filename: &HashMap<BindingId, String>,
) -> Vec<ModuleItem> {
    repair_module_imports(entry_items, "entry.js", binding_to_filename)
}

fn repair_module_imports(
    mut entry_items: Vec<ModuleItem>,
    current_filename: &str,
    binding_to_filename: &HashMap<BindingId, String>,
) -> Vec<ModuleItem> {
    let candidate_atoms: HashSet<Atom> = binding_to_filename
        .iter()
        .filter(|(_, filename)| filename.as_str() != current_filename)
        .map(|((atom, _), _)| atom.clone())
        .collect();
    if candidate_atoms.is_empty() {
        return entry_items;
    }

    let mut collector = AtomRefCollector {
        candidate_atoms: &candidate_atoms,
        references: HashSet::new(),
        shadowed_atoms: vec![HashSet::new()],
    };
    for item in &entry_items {
        item.visit_with(&mut collector);
    }

    let mut already_imported: HashSet<Atom> = entry_items
        .iter()
        .flat_map(|item| {
            module_item_import_binding_ids(item)
                .into_iter()
                .chain(module_item_declared_binding_ids(item))
        })
        .map(|(atom, _)| atom)
        .collect();
    let binding_filename_by_atom = atom_to_filename_binding_map(binding_to_filename);
    let mut imports_by_source: HashMap<String, Vec<Atom>> = HashMap::new();
    for atom in collector.references {
        if already_imported.contains(&atom) {
            continue;
        }
        let Some((_, source_filename)) = binding_filename_by_atom.get(&atom) else {
            continue;
        };
        if source_filename == current_filename {
            continue;
        }
        let specifier = relative_import_path(current_filename, source_filename);
        imports_by_source
            .entry(specifier)
            .or_default()
            .push(atom.clone());
        already_imported.insert(atom);
    }

    if imports_by_source.is_empty() {
        return entry_items;
    }

    let mut import_items = Vec::new();
    let mut sources: Vec<String> = imports_by_source.keys().cloned().collect();
    sources.sort();
    for source in sources {
        let names = imports_by_source.get_mut(&source).unwrap();
        names.sort();
        names.dedup();
        import_items.push(make_scope_import_stmt(names, &source));
    }
    import_items.append(&mut entry_items);
    import_items
}

fn augment_imports_with_referenced_atoms_for_existing_sources(
    imports_by_source: &mut HashMap<String, Vec<Atom>>,
    current_filename: &str,
    referenced_atoms: &HashSet<Atom>,
    binding_filename_by_atom: &HashMap<Atom, (BindingId, String)>,
    local_atoms: Option<&HashSet<Atom>>,
) {
    for atom in referenced_atoms {
        if local_atoms.is_some_and(|atoms| atoms.contains(atom)) {
            continue;
        }
        let Some((source_binding, source_filename)) = binding_filename_by_atom.get(atom) else {
            continue;
        };
        if source_filename == current_filename || !imports_by_source.contains_key(source_filename) {
            continue;
        }
        imports_by_source
            .entry(source_filename.clone())
            .or_default()
            .push(source_binding.0.clone());
    }
}

fn atom_to_filename_binding_map(
    bindings: &HashMap<BindingId, String>,
) -> HashMap<Atom, (BindingId, String)> {
    let mut by_atom = HashMap::new();
    for (binding, filename) in bindings {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| (binding.clone(), filename.clone()));
    }
    by_atom
}

fn atom_binding_map_from_keys<T>(imports: &HashMap<BindingId, T>) -> HashMap<Atom, BindingId> {
    let mut by_atom = HashMap::new();
    for binding in imports.keys() {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| binding.clone());
    }
    by_atom
}

fn add_factory_atom_import(
    imports_by_filename: &mut HashMap<String, Vec<BindingId>>,
    current_filename: &str,
    source_binding: &BindingId,
    source_filename: &str,
) {
    if source_filename == current_filename {
        return;
    }
    imports_by_filename
        .entry(source_filename.to_string())
        .or_default()
        .push(source_binding.clone());
}

fn atom_to_module_binding_map(
    bindings: &HashMap<BindingId, usize>,
) -> HashMap<Atom, (BindingId, usize)> {
    let mut by_atom = HashMap::new();
    for (binding, module_index) in bindings {
        by_atom
            .entry(binding.0.clone())
            .or_insert_with(|| (binding.clone(), *module_index));
    }
    by_atom
}

// ---------------------------------------------------------------------------
// Extracted factory info
// ---------------------------------------------------------------------------

struct Factory {
    /// Lazy helper used by this factory declaration.
    helper_sym: Atom,
    /// Resolved top-level binding for the factory variable.
    binding: BindingId,
    /// The declared variable name (e.g. `BO7`).
    var_name: Atom,
    /// Derived filename: filepath string key when available, else `<var_name>.js`.
    filename: String,
    /// CommonJS factory callback params: `(exports, module) => { ... }`.
    cjs_params: Option<CjsFactoryParams>,
    /// The statements inside the factory function body (unresolved — for emission).
    body_stmts: Vec<Stmt>,
    /// The statements inside the factory function body (resolved — for reference collection).
    analysis_body_stmts: Vec<Stmt>,
    /// Span of the factory's `var` declarator in the original bundle (provenance).
    span: Span,
}

#[derive(Clone)]
struct CjsFactoryParams {
    exports: Atom,
    module: Option<Atom>,
}

// ---------------------------------------------------------------------------
// Helper detection
//
// esbuild emits lazy-module helpers as top-level `var` declarations whose RHS
// is an arrow function that takes ≤2 params and *returns* another function
// (either an arrow or a named `function` expression).  Both minified and
// non-minified forms share this shape:
//
//   Minified:     (q, K) => () => ...
//   Non-minified: (cb, mod) => function __require() { ... }
// ---------------------------------------------------------------------------

fn collect_helper_syms(module: &Module) -> HashSet<Atom> {
    let mut syms = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = &decl.init else { continue };
            if is_lazy_helper(init) {
                if let Pat::Ident(bi) = &decl.name {
                    syms.insert(bi.id.sym.clone());
                }
            }
        }
    }
    syms
}

fn collect_commonjs_helper_syms(module: &Module) -> HashSet<Atom> {
    let mut syms = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = &decl.init else { continue };
            if is_lazy_helper(init) && expr_mentions_exports_member(init) {
                if let Pat::Ident(bi) = &decl.name {
                    syms.insert(bi.id.sym.clone());
                }
            }
        }
    }
    syms
}

/// Returns `true` if `expr` matches the esbuild lazy-helper shape:
///   Arrow(≤2 params) → body is Arrow or named Fn expression
fn is_lazy_helper(expr: &Expr) -> bool {
    let Expr::Arrow(outer) = expr else {
        return false;
    };
    if outer.params.len() > 2 {
        return false;
    }
    let body_expr = match &*outer.body {
        BlockStmtOrExpr::Expr(e) => e,
        BlockStmtOrExpr::BlockStmt(_) => return false,
    };
    matches!(**body_expr, Expr::Arrow(_) | Expr::Fn(_))
}

fn expr_mentions_exports_member(expr: &Expr) -> bool {
    struct ExportsMemberVisitor {
        found: bool,
    }

    impl Visit for ExportsMemberVisitor {
        fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
            if self.found {
                return;
            }
            if let swc_core::ecma::ast::MemberProp::Ident(prop) = &expr.prop {
                if prop.sym == *"exports" {
                    self.found = true;
                    return;
                }
            }
            expr.obj.visit_with(self);
            if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
                c.visit_with(self);
            }
        }

        fn visit_prop_name(&mut self, name: &PropName) {
            if self.found {
                return;
            }
            match name {
                PropName::Ident(id) if id.sym == *"exports" => {
                    self.found = true;
                }
                PropName::Computed(c) => c.visit_with(self),
                _ => {}
            }
        }
    }

    let mut visitor = ExportsMemberVisitor { found: false };
    expr.visit_with(&mut visitor);
    visitor.found
}

// ---------------------------------------------------------------------------
// Factory collection
//
// A factory is a top-level `var X = helper(fn_or_obj)` where `helper` is one
// of the detected lazy-helper symbols.
//
// Non-minified form uses an object literal whose key is the original file path:
//   var require_foo = __commonJS({ "src/foo.js"(exports, module) { … } })
//
// Minified form uses a plain arrow/function:
//   var BO7 = y(() => { … })
// ---------------------------------------------------------------------------

struct PathCommentHints<'a> {
    source: &'a str,
    source_start_pos: u32,
    hints: Vec<(usize, String)>,
}

impl<'a> PathCommentHints<'a> {
    fn new(source: &'a str, source_start_pos: u32) -> Self {
        let mut hints = Vec::new();
        let mut offset = 0usize;
        for line in source.split_inclusive('\n') {
            let text = line.trim_end_matches(['\r', '\n']);
            if let Some(path) = parse_path_comment(text) {
                hints.push((offset + line.len(), sanitize_path_comment_hint(path)));
            }
            offset += line.len();
        }
        Self {
            source,
            source_start_pos,
            hints,
        }
    }

    fn hint_before(&self, abs_byte_pos: u32) -> Option<String> {
        let rel = abs_byte_pos.checked_sub(self.source_start_pos)? as usize;
        if rel > self.source.len() {
            return None;
        }
        let (code_start, filename) = self
            .hints
            .iter()
            .rev()
            .find(|(code_start, _)| *code_start <= rel)?;
        if self.source[*code_start..rel].trim().is_empty() {
            Some(filename.clone())
        } else {
            None
        }
    }
}

fn parse_path_comment(line: &str) -> Option<String> {
    let path = line.trim_start().strip_prefix("// ")?;
    if path.starts_with('#') || path.starts_with("===") {
        return None;
    }
    let normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let valid_ext = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    valid_ext.then_some(normalized)
}

fn sanitize_path_comment_hint(path: String) -> String {
    let mut path = sanitize_path(path);
    if let Some(dot) = path.rfind('.') {
        path.replace_range(dot.., ".js");
    } else {
        path.push_str(".js");
    }
    path
}

fn collect_factories(
    module: &Module,
    analysis_module: &Module,
    helper_syms: &HashSet<Atom>,
    commonjs_helper_syms: &HashSet<Atom>,
    filename_hints: Option<&PathCommentHints<'_>>,
) -> Vec<Factory> {
    let mut factories = Vec::new();
    for (item, analysis_item) in module.body.iter().zip(analysis_module.body.iter()) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(analysis_var))) = analysis_item else {
            continue;
        };
        for (decl, analysis_decl) in var.decls.iter().zip(analysis_var.decls.iter()) {
            if let Some(factory) = try_extract_factory(
                decl,
                analysis_decl,
                var.span.lo.0,
                helper_syms,
                commonjs_helper_syms,
                filename_hints,
            ) {
                factories.push(factory);
            }
        }
    }
    factories
}

fn try_extract_factory(
    decl: &VarDeclarator,
    analysis_decl: &VarDeclarator,
    decl_start_abs: u32,
    helper_syms: &HashSet<Atom>,
    commonjs_helper_syms: &HashSet<Atom>,
    filename_hints: Option<&PathCommentHints<'_>>,
) -> Option<Factory> {
    let Pat::Ident(var_ident) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = &**init else {
        return None;
    };

    // Callee must be one of the detected helpers.
    let helper_sym = call_target_helper_sym(call, helper_syms)?;
    let is_commonjs_factory = commonjs_helper_syms.contains(&helper_sym);

    if call.args.len() != 1 {
        return None;
    }

    let arg = &*call.args[0].expr;
    let var_name = var_ident.id.sym.clone();
    let hinted_filename = filename_hints.and_then(|hints| hints.hint_before(decl_start_abs));
    let binding = match &analysis_decl.name {
        Pat::Ident(bi) => (bi.id.sym.clone(), bi.id.ctxt),
        _ => return None,
    };

    // Extract analysis (resolved) body stmts in parallel.
    let analysis_arg = analysis_decl.init.as_ref().and_then(|init| match &**init {
        Expr::Call(c) if c.args.len() == 1 => Some(&*c.args[0].expr),
        _ => None,
    });

    match arg {
        // Non-minified: __commonJS({ "src/foo.js"(exports, module) { … } })
        Expr::Object(obj) if obj.props.len() == 1 => {
            use swc_core::ecma::ast::{Prop, PropOrSpread};
            if let PropOrSpread::Prop(prop) = &obj.props[0] {
                if let Prop::Method(method) = &**prop {
                    let filename = prop_key_str(&method.key)
                        .map(sanitize_path)
                        .or_else(|| hinted_filename.clone())
                        .unwrap_or_else(|| format!("{var_name}.js"));
                    let body_stmts = method.function.body.as_ref()?.stmts.clone();
                    let analysis_body_stmts = extract_analysis_body_stmts_obj(analysis_arg)
                        .unwrap_or_else(|| body_stmts.clone());
                    return Some(Factory {
                        helper_sym,
                        binding,
                        var_name,
                        filename,
                        cjs_params: is_commonjs_factory
                            .then(|| function_cjs_params(&method.function))
                            .flatten(),
                        body_stmts,
                        analysis_body_stmts,
                        span: decl.span,
                    });
                }
            }
            None
        }

        // Minified arrow: y(() => { … }) or y(() => expr)
        Expr::Arrow(arrow) => {
            let body_stmts = arrow_body_stmts(arrow);
            let analysis_body_stmts = analysis_arg
                .and_then(|a| match a {
                    Expr::Arrow(aa) => Some(arrow_body_stmts(aa)),
                    _ => None,
                })
                .unwrap_or_else(|| body_stmts.clone());
            let filename = hinted_filename.unwrap_or_else(|| format!("{var_name}.js"));
            Some(Factory {
                helper_sym,
                binding,
                var_name,
                filename,
                cjs_params: is_commonjs_factory
                    .then(|| arrow_cjs_params(arrow))
                    .flatten(),
                body_stmts,
                analysis_body_stmts,
                span: decl.span,
            })
        }

        // Minified function: m(function() { … })
        Expr::Fn(fn_expr) => {
            let body_stmts = fn_expr.function.body.as_ref()?.stmts.clone();
            let analysis_body_stmts = analysis_arg
                .and_then(|a| match a {
                    Expr::Fn(af) => af.function.body.as_ref().map(|b| b.stmts.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| body_stmts.clone());
            let filename = hinted_filename.unwrap_or_else(|| format!("{var_name}.js"));
            Some(Factory {
                helper_sym,
                binding,
                var_name,
                filename,
                cjs_params: is_commonjs_factory
                    .then(|| function_cjs_params(&fn_expr.function))
                    .flatten(),
                body_stmts,
                analysis_body_stmts,
                span: decl.span,
            })
        }

        _ => None,
    }
}

/// Extract resolved body stmts from an analysis object-form factory argument.
fn extract_analysis_body_stmts_obj(analysis_arg: Option<&Expr>) -> Option<Vec<Stmt>> {
    let Expr::Object(obj) = analysis_arg? else {
        return None;
    };
    if obj.props.len() != 1 {
        return None;
    }
    let swc_core::ecma::ast::PropOrSpread::Prop(prop) = &obj.props[0] else {
        return None;
    };
    let swc_core::ecma::ast::Prop::Method(method) = &**prop else {
        return None;
    };
    method.function.body.as_ref().map(|b| b.stmts.clone())
}

fn arrow_cjs_params(arrow: &ArrowExpr) -> Option<CjsFactoryParams> {
    if arrow.params.is_empty() {
        return None;
    }
    let exports_name = pat_ident_atom(&arrow.params[0])?;
    let module_name = arrow.params.get(1).and_then(pat_ident_atom);
    Some(CjsFactoryParams {
        exports: exports_name,
        module: module_name,
    })
}

fn function_cjs_params(function: &Function) -> Option<CjsFactoryParams> {
    if function.params.is_empty() {
        return None;
    }
    let exports_name = pat_ident_atom(&function.params[0].pat)?;
    let module_name = function
        .params
        .get(1)
        .and_then(|param| pat_ident_atom(&param.pat));
    Some(CjsFactoryParams {
        exports: exports_name,
        module: module_name,
    })
}

fn pat_ident_atom(pat: &Pat) -> Option<Atom> {
    match pat {
        Pat::Ident(ident) => Some(ident.id.sym.clone()),
        _ => None,
    }
}

fn call_target_helper_sym(call: &CallExpr, helper_syms: &HashSet<Atom>) -> Option<Atom> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(ident) = &**callee else {
        return None;
    };
    if helper_syms.contains(&ident.sym) {
        Some(ident.sym.clone())
    } else {
        None
    }
}

fn arrow_body_stmts(arrow: &ArrowExpr) -> Vec<Stmt> {
    match &*arrow.body {
        BlockStmtOrExpr::BlockStmt(block) => block.stmts.clone(),
        BlockStmtOrExpr::Expr(expr) => vec![Stmt::Expr(ExprStmt {
            span: Default::default(),
            expr: expr.clone(),
        })],
    }
}

fn prop_key_str(key: &swc_core::ecma::ast::PropName) -> Option<String> {
    use swc_core::ecma::ast::PropName;
    match key {
        PropName::Str(Str { value, .. }) => Some(value.as_str().unwrap_or("").to_string()),
        PropName::Ident(id) => Some(id.sym.to_string()),
        _ => None,
    }
}

/// Convert a source-map style path (`../src/foo.js`, `webpack:///src/foo.js`) to a
/// safe relative path suitable as a filename.
fn sanitize_path(raw: String) -> String {
    let s = raw
        .trim_start_matches("webpack://")
        .trim_start_matches("webpack:///")
        .trim_start_matches('/');
    crate::unpacker::sanitize_relative_path(s, "module.js")
}

fn filter_helper_factory_declarators(
    item: &ModuleItem,
    helper_factory_syms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
        return Some(item.clone());
    };
    let mut filtered = var_decl.clone();
    filtered.decls.retain(|decl| {
        !matches!(
            &decl.name,
            Pat::Ident(bi) if helper_factory_syms.contains(&bi.id.sym)
        )
    });
    if filtered.decls.is_empty() {
        None
    } else {
        Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))))
    }
}

fn item_has_helper_factory_declarator(
    item: &ModuleItem,
    helper_factory_syms: &HashSet<Atom>,
) -> bool {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
        return false;
    };
    var_decl.decls.iter().any(|decl| {
        matches!(
            &decl.name,
            Pat::Ident(bi) if helper_factory_syms.contains(&bi.id.sym)
        )
    })
}

// ---------------------------------------------------------------------------
// Scope-hoisted module extraction
//
// esbuild scope-hoists ESM modules into a flat top-level scope. Each
// scope-hoisted module is marked by:
//
//   var NS = {};
//   __export(NS, { exportName: () => localBinding, ... });
//   ... module code (var/function/class declarations) ...
//
// The `__export` helper is an arrow:
//   (target, all) => { for (var name in all) defProp(target, name, {get: all[name], ...}) }
//
// KNOWN LIMITATION (last-module boundary):
// For non-last modules, the next `var NS = {}; __export(NS, ...)` boundary
// cleanly delimits module code. For the last module, we use a three-phase
// heuristic: Phase 1 finds the last exported-binding declaration, Phase 2
// extends via reference closure (private helpers after exports), Phase 3
// includes trailing expression statements that reference module bindings.
//
// This can misattribute entry-level expressions that reference bindings
// from the last module. For example:
//   // constants.js (module side effect)
//   console.log(LABEL, VALUE);
//   // entry.js (entry code referencing same binding)
//   console.log("entry", VALUE);
//
// Both appear after the last export and reference `VALUE`. In minified
// production bundles there is no structural marker distinguishing them —
// the ambiguity is inherent. The misattribution is cosmetic (code lands
// in the wrong file) not functional (bindings remain accessible in the
// shared scope).
//
// We detect this helper, find all namespace+export pairs, and partition
// the top-level items into per-module groups.
// ---------------------------------------------------------------------------

/// Metadata collected during the first pass over scope-hoisted boundaries.
#[derive(Clone)]
struct ScopeNamespaceExport {
    namespace_binding: BindingId,
    export_entries: Vec<(Atom, BindingId)>,
}

#[derive(Clone)]
struct ScopeModuleMeta {
    namespaces: Vec<ScopeNamespaceExport>,
    body_indices: Vec<usize>,
    owned_support_bindings: HashSet<BindingId>,
    exported_bindings: HashSet<BindingId>,
    exported_atoms: HashSet<Atom>,
    declared_bindings: HashSet<BindingId>,
    local_import_bindings: HashSet<BindingId>,
    referenced_bindings: HashSet<BindingId>,
    written_atoms: HashSet<Atom>,
    referenced_atoms: HashSet<Atom>,
    filename: String,
    id: String,
}

struct ScopeExtractionRefs<'a> {
    factory_referenced: &'a HashSet<BindingId>,
    factory_preassigned_bindings: &'a HashMap<BindingId, String>,
    factory_importable_bindings: &'a HashMap<BindingId, String>,
    drop_unowned_helper_sibling_indices: &'a HashSet<usize>,
}

fn merge_conflicting_factory_scope_metas(
    metas: &mut Vec<ScopeModuleMeta>,
    factory_preassigned_by_atom: &HashMap<Atom, (BindingId, String)>,
) {
    if metas.len() < 2 {
        return;
    }

    // A lazy init factory can seed mutable bindings that are later assigned by
    // different scope modules. Those modules must stay in one synthetic ESM
    // file with the factory, otherwise one writer would assign to an import.
    let mut writer_modules_by_factory: HashMap<String, Vec<usize>> = HashMap::new();
    for (mi, meta) in metas.iter().enumerate() {
        let mut factories = HashSet::new();
        for atom in &meta.written_atoms {
            let Some((_, factory_filename)) = factory_preassigned_by_atom.get(atom) else {
                continue;
            };
            if *factory_filename != meta.filename {
                factories.insert(factory_filename.clone());
            }
        }
        for factory_filename in factories {
            writer_modules_by_factory
                .entry(factory_filename)
                .or_default()
                .push(mi);
        }
    }

    let mut adjacency: Vec<HashSet<usize>> = vec![HashSet::new(); metas.len()];
    for writers in writer_modules_by_factory.values_mut() {
        writers.sort_unstable();
        writers.dedup();
        if writers.len() < 2 {
            continue;
        }
        let first = writers[0];
        for &writer in &writers[1..] {
            adjacency[first].insert(writer);
            adjacency[writer].insert(first);
        }
    }

    if adjacency.iter().all(HashSet::is_empty) {
        return;
    }

    let mut group_by_first: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut member_to_first: HashMap<usize, usize> = HashMap::new();
    let mut visited = vec![false; metas.len()];
    for start in 0..metas.len() {
        if visited[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut group = Vec::new();
        visited[start] = true;
        while let Some(current) = stack.pop() {
            group.push(current);
            for &next in &adjacency[current] {
                if visited[next] {
                    continue;
                }
                visited[next] = true;
                stack.push(next);
            }
        }
        if group.len() < 2 {
            continue;
        }
        group.sort_unstable();
        let first = group[0];
        for &member in &group {
            member_to_first.insert(member, first);
        }
        group_by_first.insert(first, group);
    }

    if group_by_first.is_empty() {
        return;
    }

    let original = metas.clone();
    let mut merged = Vec::new();
    for index in 0..original.len() {
        if member_to_first
            .get(&index)
            .is_some_and(|first| *first != index)
        {
            continue;
        }
        if let Some(group) = group_by_first.get(&index) {
            merged.push(merge_scope_meta_group(&original, group));
        } else {
            merged.push(original[index].clone());
        }
    }
    *metas = merged;
}

fn merge_scope_meta_group(metas: &[ScopeModuleMeta], group: &[usize]) -> ScopeModuleMeta {
    let mut merged = metas[group[0]].clone();
    for &index in &group[1..] {
        let meta = &metas[index];
        merged.namespaces.extend(meta.namespaces.clone());
        merged
            .body_indices
            .extend(meta.body_indices.iter().copied());
        merged
            .owned_support_bindings
            .extend(meta.owned_support_bindings.iter().cloned());
        merged
            .exported_bindings
            .extend(meta.exported_bindings.iter().cloned());
        merged
            .exported_atoms
            .extend(meta.exported_atoms.iter().cloned());
        merged
            .declared_bindings
            .extend(meta.declared_bindings.iter().cloned());
        merged
            .local_import_bindings
            .extend(meta.local_import_bindings.iter().cloned());
        merged
            .referenced_bindings
            .extend(meta.referenced_bindings.iter().cloned());
        merged
            .written_atoms
            .extend(meta.written_atoms.iter().cloned());
        merged
            .referenced_atoms
            .extend(meta.referenced_atoms.iter().cloned());
    }
    merged.body_indices.sort_unstable();
    merged.body_indices.dedup();
    merged
}

/// Extract scope-hoisted modules from entry items.
/// Returns (extracted_modules, remaining_entry_items, binding_to_filename).
///
/// After partitioning items into per-module groups, this function
/// synthesizes ES import/export statements so that cross-module
/// references (which the bundler resolved via direct bindings) are
/// represented as standard module edges.
///
/// `seen_lower` is the shared case-insensitive filename set, already
/// populated by factory modules.  Scope-hoisted filenames are probed
/// against it so they never collide with factories or each other.
///
/// `factory_referenced` contains all bindings referenced by factory modules.
/// These are included in export expansion so scope-hoisted modules export
/// bindings that factories need.  The returned `binding_to_filename` map
/// lets callers synthesize imports in factory modules.
fn extract_scope_hoisted_modules(
    analysis_items: &[ModuleItem],
    source_items: Vec<ModuleItem>,
    seen_lower: &mut HashSet<String>,
    cm: Lrc<SourceMap>,
    refs: ScopeExtractionRefs<'_>,
) -> (
    Vec<UnpackedModule>,
    Vec<ModuleItem>,
    HashMap<BindingId, String>,
    HashMap<String, HashSet<BindingId>>,
    HashMap<String, HashSet<Atom>>,
    HashMap<String, HashSet<Atom>>,
    HashMap<BindingId, String>,
) {
    debug_assert_eq!(analysis_items.len(), source_items.len());
    let ScopeExtractionRefs {
        factory_referenced,
        factory_preassigned_bindings,
        factory_importable_bindings,
        drop_unowned_helper_sibling_indices,
    } = refs;

    let span = tracing::info_span!("esbuild: scope collect metadata");
    let metadata_enter = span.enter();

    // Step 1: find the __export helper binding.
    let Some((export_helper_index, export_helper)) = detect_export_helper(analysis_items) else {
        return (
            vec![],
            source_items,
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
    };
    let item_infos = build_item_binding_infos(analysis_items);
    let top_level_bindings: HashSet<BindingId> = item_infos
        .iter()
        .flat_map(|info| info.declared.iter().cloned())
        .chain(
            analysis_items
                .iter()
                .flat_map(|item| module_item_import_binding_ids(item).into_iter()),
        )
        .collect();
    let top_level_atoms: HashSet<Atom> = top_level_bindings
        .iter()
        .map(|(atom, _)| atom.clone())
        .collect();
    let external_imports = collect_external_imports(analysis_items, &source_items);

    // Step 2: find all (namespace_decl_index, export_call_index, ns_atom) triples.
    let boundaries = collect_scope_hoisted_boundaries(analysis_items, &export_helper);
    if boundaries.is_empty() {
        return (
            vec![],
            source_items,
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
    }
    drop(metadata_enter);

    // Convert to Option<ModuleItem> so items can be moved out by index.
    let mut source_slots: Vec<Option<ModuleItem>> = source_items.into_iter().map(Some).collect();
    let original_source_items: Vec<ModuleItem> = source_slots
        .iter()
        .map(|item| item.as_ref().expect("source slot should exist").clone())
        .collect();

    // Step 3 (pass 1): partition items and collect per-module metadata.
    let span = tracing::info_span!("esbuild: scope partition modules", count = boundaries.len());
    let partition_enter = span.enter();
    let mut metas: Vec<ScopeModuleMeta> = Vec::new();
    let mut consumed: HashSet<usize> = HashSet::new();
    let scope_candidate_atoms: HashSet<Atom> = item_infos
        .iter()
        .flat_map(|info| info.declared.iter().map(|(atom, _)| atom.clone()))
        .collect();
    let factory_preassigned_set: HashSet<BindingId> =
        factory_preassigned_bindings.keys().cloned().collect();
    let factory_preassigned_atoms: HashSet<Atom> = factory_preassigned_bindings
        .keys()
        .map(|(atom, _)| atom.clone())
        .collect();
    let mut reference_candidate_atoms = scope_candidate_atoms.clone();
    reference_candidate_atoms.extend(factory_preassigned_atoms.iter().cloned());
    reference_candidate_atoms.extend(
        factory_importable_bindings
            .keys()
            .map(|(atom, _)| atom.clone()),
    );

    // Track consumed namespace bindings so we can restore them for the entry.
    let mut consumed_ns: Vec<(usize, usize, &ScopeHoistedBoundary)> = Vec::new();

    let removable_export_helper_indices = removable_export_helper_dependency_indices(
        export_helper_index,
        analysis_items,
        &item_infos,
        &boundaries,
    );
    consumed.extend(removable_export_helper_indices.iter().copied());

    // Collect all factory-referenced atoms (not BindingIds) so we can use
    // them when finding the last module's end boundary.  This ensures private
    // helpers only referenced by factories are absorbed into the scope-hoisted
    // module rather than leaking into entry.js.
    let factory_referenced_atoms: HashSet<Atom> = factory_referenced
        .iter()
        .map(|(atom, _)| atom.clone())
        .collect();

    for (bi, boundary) in boundaries.iter().enumerate() {
        let start = boundary.ns_decl_index;
        let end = if bi + 1 < boundaries.len() {
            boundaries[bi + 1].ns_decl_index
        } else {
            find_last_module_end(
                analysis_items,
                &item_infos,
                boundary.export_call_index + 1,
                &boundary.exported_bindings,
                &factory_referenced_atoms,
            )
        };

        let mut body_indices: Vec<usize> = Vec::new();
        let mut declared_bindings: HashSet<BindingId> = HashSet::new();
        let mut local_import_bindings: HashSet<BindingId> = HashSet::new();
        let mut referenced_bindings: HashSet<BindingId> = HashSet::new();
        let mut written_atoms: HashSet<Atom> = HashSet::new();
        let mut referenced_atoms: HashSet<Atom> = HashSet::new();

        for i in start..end {
            consumed.insert(i);
            if i == boundary.ns_decl_index || i == boundary.export_call_index {
                continue;
            }
            let item_needs_filtering = item_infos[i].declared.iter().any(|id| {
                factory_preassigned_set.contains(id) || factory_preassigned_atoms.contains(&id.0)
            });
            let mut filtered_analysis_item = None;
            let mut filtered_info = None;
            if item_needs_filtering {
                let Some(item) = filter_item_excluding_bindings(
                    &analysis_items[i],
                    &factory_preassigned_set,
                    &factory_preassigned_atoms,
                ) else {
                    continue;
                };
                let filtered_source_item = source_slots[i]
                    .as_ref()
                    .and_then(|source_item| {
                        filter_item_excluding_bindings(
                            source_item,
                            &factory_preassigned_set,
                            &factory_preassigned_atoms,
                        )
                    })
                    .expect("source item should filter with analysis item");
                source_slots[i] = Some(filtered_source_item);
                filtered_info = Some(item_binding_info_for(&item, &top_level_bindings));
                filtered_analysis_item = Some(item);
            }
            let analysis_item_for_visits = filtered_analysis_item
                .as_ref()
                .unwrap_or(&analysis_items[i]);
            let info = filtered_info.as_ref().unwrap_or(&item_infos[i]);

            let mut atom_collector = AtomRefCollector {
                candidate_atoms: &reference_candidate_atoms,
                references: HashSet::new(),
                shadowed_atoms: vec![HashSet::new()],
            };
            analysis_item_for_visits.visit_with(&mut atom_collector);
            body_indices.push(i);
            declared_bindings.extend(info.declared.iter().cloned());
            local_import_bindings.extend(module_item_import_binding_ids(analysis_item_for_visits));
            referenced_bindings.extend(info.references.iter().cloned());
            written_atoms.extend(scope_write_atoms_for_item(
                analysis_item_for_visits,
                &top_level_bindings,
                &top_level_atoms,
            ));
            referenced_atoms.extend(atom_collector.references);
        }

        consumed_ns.push((boundary.ns_decl_index, boundary.export_call_index, boundary));

        if body_indices.is_empty() {
            continue;
        }

        let exported_atoms: HashSet<Atom> = boundary
            .exported_bindings
            .iter()
            .map(|(atom, _)| atom.clone())
            .collect();

        let base_name = boundary.ns_atom.to_string();
        let filename = dedup_filename(&format!("{base_name}.js"), seen_lower);
        let id = filename
            .strip_suffix(".js")
            .unwrap_or(&filename)
            .to_string();

        metas.push(ScopeModuleMeta {
            namespaces: vec![ScopeNamespaceExport {
                namespace_binding: boundary.ns_binding.clone(),
                export_entries: boundary.export_entries.clone(),
            }],
            body_indices,
            owned_support_bindings: HashSet::new(),
            exported_bindings: boundary.exported_bindings.clone(),
            exported_atoms,
            declared_bindings,
            local_import_bindings,
            referenced_bindings,
            written_atoms,
            referenced_atoms,
            filename,
            id,
        });
    }
    drop(partition_enter);

    let span = tracing::info_span!("esbuild: scope build binding maps", count = metas.len());
    let binding_maps_enter = span.enter();
    let factory_preassigned_by_atom = atom_to_filename_binding_map(factory_preassigned_bindings);
    merge_conflicting_factory_scope_metas(&mut metas, &factory_preassigned_by_atom);

    // Build binding → module index map for all scope-hoisted modules.
    let mut binding_to_module: HashMap<BindingId, usize> = HashMap::new();
    let mut module_local_atoms: HashMap<String, HashSet<Atom>> = HashMap::new();
    for (mi, meta) in metas.iter().enumerate() {
        for namespace in &meta.namespaces {
            binding_to_module.insert(namespace.namespace_binding.clone(), mi);
        }
        for binding in &meta.declared_bindings {
            binding_to_module.insert(binding.clone(), mi);
        }
    }

    let decl_index_by_binding: HashMap<BindingId, usize> = item_infos
        .iter()
        .enumerate()
        .flat_map(|(index, info)| {
            info.declared
                .iter()
                .cloned()
                .map(move |binding| (binding, index))
        })
        .collect();
    drop(binding_maps_enter);

    // Scope-hoisted modules often call small top-level helpers that sit before
    // the namespace block. Move safe helper-like declarations into the first
    // extracted module that needs them so generated modules don't reference
    // invisible bindings left behind in entry.js.
    let span = tracing::info_span!("esbuild: scope adopt support decls");
    let support_enter = span.enter();
    let mut owned_support_by_index: HashMap<usize, HashSet<BindingId>> = HashMap::new();
    for (mi, meta) in metas.iter_mut().enumerate() {
        let module_start = meta
            .body_indices
            .iter()
            .min()
            .copied()
            .unwrap_or(usize::MAX);
        let mut queue: Vec<BindingId> = meta
            .referenced_bindings
            .iter()
            .chain(meta.exported_bindings.iter())
            .cloned()
            .collect();
        while let Some(binding) = queue.pop() {
            if meta.declared_bindings.contains(&binding)
                || binding_to_module.contains_key(&binding)
                || factory_preassigned_bindings.contains_key(&binding)
                || factory_importable_bindings.contains_key(&binding)
                || external_imports.contains_key(&binding)
                || meta.local_import_bindings.contains(&binding)
            {
                continue;
            }

            let Some(&decl_index) = decl_index_by_binding.get(&binding) else {
                continue;
            };
            if consumed.contains(&decl_index)
                || decl_index >= module_start
                || !is_scope_support_declaration_for_binding(&analysis_items[decl_index], &binding)
            {
                continue;
            }

            binding_to_module.insert(binding.clone(), mi);
            meta.declared_bindings.insert(binding.clone());
            meta.owned_support_bindings.insert(binding.clone());
            owned_support_by_index
                .entry(decl_index)
                .or_default()
                .insert(binding.clone());

            for ref_binding in &item_infos[decl_index].references {
                if meta.referenced_bindings.insert(ref_binding.clone()) {
                    queue.push(ref_binding.clone());
                }
            }

            let mut atom_collector = AtomRefCollector {
                candidate_atoms: &reference_candidate_atoms,
                references: HashSet::new(),
                shadowed_atoms: vec![HashSet::new()],
            };
            analysis_items[decl_index].visit_with(&mut atom_collector);
            meta.referenced_atoms.extend(atom_collector.references);
        }
    }

    for (index, owned) in &owned_support_by_index {
        let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
        if let Some(item) = source_slots[*index].take() {
            source_slots[*index] = filter_item_excluding_bindings(&item, owned, &owned_atoms);
        }
        if source_slots[*index].is_none() {
            consumed.insert(*index);
        }
    }
    for index in drop_unowned_helper_sibling_indices {
        if source_slots.get(*index).and_then(Option::as_ref).is_some() {
            source_slots[*index] = None;
            consumed.insert(*index);
        }
    }
    drop(support_enter);

    let span = tracing::info_span!("esbuild: scope compute imports exports");
    let imports_exports_enter = span.enter();
    let binding_module_by_atom = atom_to_module_binding_map(&binding_to_module);

    // Collect remaining entry references early so they feed into the
    // effective-export expansion below.
    let remaining_indices: Vec<usize> = (0..source_slots.len())
        .filter(|i| !consumed.contains(i))
        .collect();

    let mut entry_referenced: HashSet<BindingId> = HashSet::new();
    for &i in &remaining_indices {
        entry_referenced.extend(item_infos[i].references.iter().cloned());
    }
    for &(ns_idx, call_idx, _) in &consumed_ns {
        entry_referenced.extend(item_infos[ns_idx].references.iter().cloned());
        entry_referenced.extend(item_infos[call_idx].references.iter().cloned());
    }

    // Expand export sets: the T8-registered exports are the module's public
    // API, but the bundler's scope hoisting lets other modules directly
    // reference private helpers too.  Any declared binding referenced from
    // outside (by another module OR by the entry) must be exported.
    let mut effective_exports: Vec<HashSet<Atom>> =
        metas.iter().map(|m| m.exported_atoms.clone()).collect();
    for (mi, meta) in metas.iter().enumerate() {
        for ref_binding in &meta.referenced_bindings {
            if meta.declared_bindings.contains(ref_binding) {
                continue;
            }
            if let Some(&source_mi) = binding_to_module.get(ref_binding) {
                if source_mi != mi {
                    effective_exports[source_mi].insert(ref_binding.0.clone());
                }
            }
        }
        for export_binding in &meta.exported_bindings {
            if meta.declared_bindings.contains(export_binding) {
                continue;
            }
            if let Some(&source_mi) = binding_to_module.get(export_binding) {
                if source_mi != mi {
                    effective_exports[source_mi].insert(export_binding.0.clone());
                }
            }
        }
    }
    for ref_binding in &entry_referenced {
        if let Some(&source_mi) = binding_to_module.get(ref_binding) {
            effective_exports[source_mi].insert(ref_binding.0.clone());
        }
    }
    // Also expand for references from factory modules.
    for ref_binding in factory_referenced {
        if let Some(&source_mi) = binding_to_module.get(ref_binding) {
            effective_exports[source_mi].insert(ref_binding.0.clone());
        }
    }

    let mut claimed_factory_filenames: HashMap<String, String> = HashMap::new();
    let mut conflicted_factory_filenames: HashSet<String> = HashSet::new();
    for meta in &metas {
        for atom in &meta.written_atoms {
            let Some((_, factory_filename)) = factory_preassigned_by_atom.get(atom) else {
                continue;
            };
            if *factory_filename == meta.filename {
                continue;
            }
            match claimed_factory_filenames.get(factory_filename) {
                Some(existing) if existing != &meta.filename => {
                    conflicted_factory_filenames.insert(factory_filename.clone());
                }
                Some(_) => {}
                None => {
                    claimed_factory_filenames
                        .insert(factory_filename.clone(), meta.filename.clone());
                }
            }
        }
    }
    for filename in &conflicted_factory_filenames {
        claimed_factory_filenames.remove(filename);
    }

    // Build binding→filename map so callers can synthesize imports in factory modules.
    let mut binding_to_filename: HashMap<BindingId, String> = binding_to_module
        .iter()
        .map(|(binding, &mi)| (binding.clone(), metas[mi].filename.clone()))
        .collect();
    let mut scope_claimed_factory_bindings: HashMap<BindingId, String> = HashMap::new();
    for (binding, filename) in factory_preassigned_bindings {
        let owner_filename = claimed_factory_filenames
            .get(filename)
            .unwrap_or(filename)
            .clone();
        if &owner_filename != filename {
            scope_claimed_factory_bindings.insert(binding.clone(), owner_filename.clone());
        }
        binding_to_filename.insert(binding.clone(), owner_filename);
    }
    let factory_binding_filename_by_atom =
        atom_to_filename_binding_map(factory_importable_bindings);
    let binding_filename_by_atom = atom_to_filename_binding_map(&binding_to_filename);
    let filename_to_module: HashMap<String, usize> = metas
        .iter()
        .enumerate()
        .map(|(mi, meta)| (meta.filename.clone(), mi))
        .collect();

    // Map namespace bindings to "entry.js".  The namespace object
    // (`var ns_a = {}; __export(ns_a, {...})`) is restored into the entry
    // when the entry's own export declaration references it.  Factories
    // that use `ns_a.greet()` need to import the namespace from there.
    for boundary in &boundaries {
        if factory_referenced.contains(&boundary.ns_binding) {
            binding_to_filename
                .entry(boundary.ns_binding.clone())
                .or_insert_with(|| "entry.js".to_string());
        }
    }
    drop(imports_exports_enter);

    // Step 4 (pass 2): emit each module with synthesized imports/exports.
    let span = tracing::info_span!("esbuild: scope emit modules", count = metas.len());
    let emit_modules_enter = span.enter();
    let mut modules = Vec::new();
    let mut module_referenced_atoms: HashMap<String, HashSet<Atom>> = HashMap::new();

    for (mi, meta) in metas.iter().enumerate() {
        let mut module_items: Vec<ModuleItem> = Vec::new();

        // Synthesize imports from other scope-hoisted modules.
        let declared_atoms: HashSet<Atom> = meta
            .declared_bindings
            .iter()
            .map(|(atom, _)| atom.clone())
            .collect();
        let mut imports_by_source: HashMap<usize, Vec<BindingId>> = HashMap::new();
        let mut imports_by_filename: HashMap<String, Vec<BindingId>> = HashMap::new();
        let mut external_import_bindings: HashSet<BindingId> = HashSet::new();
        for ref_binding in &meta.referenced_bindings {
            if meta.declared_bindings.contains(ref_binding) {
                continue;
            }
            if declared_atoms.contains(&ref_binding.0) {
                continue;
            }
            if let Some(&source_mi) = binding_to_module.get(ref_binding) {
                if source_mi != mi {
                    imports_by_source
                        .entry(source_mi)
                        .or_default()
                        .push(ref_binding.clone());
                }
            } else if let Some((source_binding, source_mi)) =
                binding_module_by_atom.get(&ref_binding.0)
            {
                if *source_mi != mi {
                    imports_by_source
                        .entry(*source_mi)
                        .or_default()
                        .push(source_binding.clone());
                }
            } else if factory_preassigned_bindings.contains_key(ref_binding) {
                let source_filename = binding_to_filename
                    .get(ref_binding)
                    .expect("factory preassigned binding should have an owner filename");
                if *source_filename != meta.filename {
                    imports_by_filename
                        .entry(source_filename.clone())
                        .or_default()
                        .push(ref_binding.clone());
                }
            } else if external_imports.contains_key(ref_binding)
                && !meta.local_import_bindings.contains(ref_binding)
            {
                external_import_bindings.insert(ref_binding.clone());
            }
        }
        for atom in &meta.referenced_atoms {
            if declared_atoms.contains(atom) {
                continue;
            }
            if imports_by_source
                .values()
                .any(|bindings| bindings.iter().any(|binding| &binding.0 == atom))
            {
                continue;
            }
            // Atom fallback repairs missing specifiers on a module edge that
            // exact binding analysis already found.  Creating new edges by
            // atom alone is too broad for large bundles with reused minified
            // names and can manufacture import cycles.
            let mut added_existing_edge_import = false;
            if let Some((source_binding, source_mi)) = binding_module_by_atom.get(atom) {
                if *source_mi != mi && imports_by_source.contains_key(source_mi) {
                    imports_by_source
                        .entry(*source_mi)
                        .or_default()
                        .push(source_binding.clone());
                    added_existing_edge_import = true;
                }
            }
            if !added_existing_edge_import {
                if let Some((source_binding, source_filename)) = binding_filename_by_atom.get(atom)
                {
                    if let Some(source_mi) = filename_to_module.get(source_filename) {
                        if *source_mi != mi && imports_by_source.contains_key(source_mi) {
                            imports_by_source
                                .entry(*source_mi)
                                .or_default()
                                .push(source_binding.clone());
                            added_existing_edge_import = true;
                        }
                    }
                }
            }
            let atom_owned_by_scope_module = binding_filename_by_atom
                .get(atom)
                .is_some_and(|(_, filename)| filename_to_module.contains_key(filename));
            if !added_existing_edge_import && !atom_owned_by_scope_module {
                if let Some((source_binding, source_filename)) =
                    factory_binding_filename_by_atom.get(atom)
                {
                    add_factory_atom_import(
                        &mut imports_by_filename,
                        &meta.filename,
                        source_binding,
                        source_filename,
                    );
                }
            }
        }
        // Export getter bodies (`__export(ns, { name: () => binding })`) are
        // module surface, not body code. If a getter re-exports a binding from
        // another extracted module, import it here so the later export
        // statement does not reference an undeclared local.
        for export_binding in &meta.exported_bindings {
            if meta.declared_bindings.contains(export_binding) {
                continue;
            }
            if declared_atoms.contains(&export_binding.0) {
                continue;
            }
            if let Some(&source_mi) = binding_to_module.get(export_binding) {
                if source_mi != mi {
                    imports_by_source
                        .entry(source_mi)
                        .or_default()
                        .push(export_binding.clone());
                }
            } else if let Some((source_binding, source_mi)) =
                binding_module_by_atom.get(&export_binding.0)
            {
                if *source_mi != mi {
                    imports_by_source
                        .entry(*source_mi)
                        .or_default()
                        .push(source_binding.clone());
                }
            } else if factory_preassigned_bindings.contains_key(export_binding) {
                let source_filename = binding_to_filename
                    .get(export_binding)
                    .expect("factory preassigned binding should have an owner filename");
                if *source_filename != meta.filename {
                    imports_by_filename
                        .entry(source_filename.clone())
                        .or_default()
                        .push(export_binding.clone());
                }
            } else if external_imports.contains_key(export_binding)
                && !meta.local_import_bindings.contains(export_binding)
            {
                external_import_bindings.insert(export_binding.clone());
            }
        }
        let mut external_import_bindings: Vec<BindingId> =
            external_import_bindings.into_iter().collect();
        external_import_bindings.sort_by(|a, b| a.0.cmp(&b.0));
        let mut external_imported_atoms = HashSet::new();
        for binding in external_import_bindings {
            if declared_atoms.contains(&binding.0) {
                continue;
            }
            if let Some(import) = external_imports.get(&binding) {
                external_imported_atoms.insert(binding.0.clone());
                module_items.push(make_external_import_stmt(import));
            }
        }
        let mut import_renames: Vec<BindingRename> = Vec::new();
        let mut reserved_import_atoms = declared_atoms.clone();
        reserved_import_atoms.extend(
            binding_to_filename
                .iter()
                .filter(|(_, filename)| *filename == &meta.filename)
                .map(|((atom, _), _)| atom.clone()),
        );
        reserved_import_atoms.extend(
            meta.local_import_bindings
                .iter()
                .map(|(atom, _)| atom.clone()),
        );
        let mut import_sources: Vec<usize> = imports_by_source.keys().copied().collect();
        import_sources.sort();
        let mut imported_atoms = HashSet::new();
        for source_mi in import_sources {
            let bindings = imports_by_source.get_mut(&source_mi).unwrap();
            bindings.sort_by(|a, b| a.0.cmp(&b.0));
            bindings.dedup();
            let mut names = Vec::new();
            for binding in bindings {
                let imported = binding.0.clone();
                let local = reserve_import_atom(&imported, &mut reserved_import_atoms);
                if local != imported {
                    import_renames.push(BindingRename {
                        old: binding.clone(),
                        new: local.clone(),
                    });
                }
                imported_atoms.insert(local.clone());
                names.push((imported, local));
            }
            module_items.push(make_scope_import_stmt_with_aliases(
                &names,
                &metas[source_mi].filename,
            ));
        }
        reserved_import_atoms.extend(imported_atoms.iter().cloned());
        let mut import_filenames: Vec<String> = imports_by_filename.keys().cloned().collect();
        import_filenames.sort();
        for source_filename in import_filenames {
            let bindings = imports_by_filename.get_mut(&source_filename).unwrap();
            bindings.sort_by(|a, b| a.0.cmp(&b.0));
            bindings.dedup();
            let mut names = Vec::new();
            for binding in bindings {
                let imported = binding.0.clone();
                let local = reserve_import_atom(&imported, &mut reserved_import_atoms);
                if local != imported {
                    import_renames.push(BindingRename {
                        old: binding.clone(),
                        new: local.clone(),
                    });
                }
                imported_atoms.insert(local.clone());
                names.push((imported, local));
            }
            let rel_path = relative_import_path(&meta.filename, &source_filename);
            module_items.push(make_scope_import_stmt_with_aliases(&names, &rel_path));
        }
        imported_atoms.extend(external_imported_atoms);
        let mut local_atoms = declared_atoms.clone();
        local_atoms.extend(
            binding_to_filename
                .iter()
                .filter(|(_, filename)| *filename == &meta.filename)
                .map(|((atom, _), _)| atom.clone()),
        );
        local_atoms.extend(imported_atoms.iter().cloned());
        local_atoms.extend(
            meta.local_import_bindings
                .iter()
                .map(|(atom, _)| atom.clone()),
        );
        module_local_atoms.insert(meta.filename.clone(), local_atoms);
        module_referenced_atoms.insert(meta.filename.clone(), meta.referenced_atoms.clone());
        let namespace_atoms: HashSet<Atom> = meta
            .namespaces
            .iter()
            .map(|namespace| namespace.namespace_binding.0.clone())
            .collect();
        let exports: HashSet<Atom> = effective_exports[mi]
            .iter()
            .filter(|atom| !namespace_atoms.contains(*atom))
            .filter(|atom| declared_atoms.contains(*atom) || imported_atoms.contains(*atom))
            .cloned()
            .collect();

        // Body items with export promotion for exported bindings.
        let mut remaining_exports = exports;
        for item in scope_owned_support_decl_items(
            &meta.owned_support_bindings,
            &decl_index_by_binding,
            &original_source_items,
        ) {
            if remaining_exports.is_empty() {
                module_items.push(item);
                continue;
            }
            match try_promote_scope_export(item, &remaining_exports) {
                ScopeExportPromotion::Promoted(new_item, promoted) => {
                    module_items.push(new_item);
                    for name in &promoted {
                        remaining_exports.remove(name);
                    }
                }
                ScopeExportPromotion::Unchanged(item) => {
                    module_items.push(item);
                }
            }
        }
        let mut body_spans: Vec<Span> = Vec::with_capacity(meta.body_indices.len());
        for &i in &meta.body_indices {
            let item = source_slots[i].take().expect("body item already consumed");
            body_spans.push(item.span());
            if remaining_exports.is_empty() {
                module_items.push(item);
                continue;
            }
            match try_promote_scope_export(item, &remaining_exports) {
                ScopeExportPromotion::Promoted(new_item, promoted) => {
                    module_items.push(new_item);
                    for name in &promoted {
                        remaining_exports.remove(name);
                    }
                }
                ScopeExportPromotion::Unchanged(item) => {
                    module_items.push(item);
                }
            }
        }
        if !remaining_exports.is_empty() {
            let mut names: Vec<Atom> = remaining_exports.into_iter().collect();
            names.sort();
            module_items.push(make_scope_export_stmt(&names));
        }

        rename_bindings(&mut module_items, &import_renames);
        let mut code = emit_items(module_items, meta.filename.clone(), cm.clone());
        for namespace in &meta.namespaces {
            if !effective_exports[mi].contains(&namespace.namespace_binding.0) {
                continue;
            }
            code.push('\n');
            code.push_str(&make_namespace_export_code(
                &namespace.namespace_binding.0,
                &namespace.export_entries,
            ));
        }
        modules.push(UnpackedModule {
            id: meta.id.clone(),
            is_entry: false,
            code,
            filename: meta.filename.clone(),
            source_ranges: spans_byte_ranges(&cm, body_spans.into_iter()),
            source_input: String::new(),
        });
    }
    drop(emit_modules_enter);

    let span = tracing::info_span!("esbuild: scope build entry");
    let build_entry_enter = span.enter();
    // Track which external bindings each scope-hoisted module already imports
    // (used later to avoid duplicate imports when merging init factories).
    let mut module_already_imports: HashMap<String, HashSet<BindingId>> = HashMap::new();
    for meta in &metas {
        let imported: HashSet<BindingId> = meta
            .referenced_bindings
            .iter()
            .filter(|b| !meta.declared_bindings.contains(b))
            .filter(|b| {
                binding_to_module.contains_key(b)
                    || external_imports.contains_key(b)
                    || meta.local_import_bindings.contains(b)
            })
            .cloned()
            .collect();
        module_already_imports.insert(meta.filename.clone(), imported);
    }

    // Collect atoms that the remaining entry items already export via ESM
    // `export { ... }` declarations.  Used below to avoid synthesizing
    // duplicate exports for namespace bindings.
    // Only count unaliased exports: `export { ns_a }` makes `ns_a`
    // importable by name, but `export { ns_a as math }` does not —
    // consumers would need `import { math }`, not `import { ns_a }`.
    let entry_already_exports: HashSet<Atom> = remaining_indices
        .iter()
        .flat_map(|&i| match source_slots[i].as_ref().unwrap() {
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)) => named
                .specifiers
                .iter()
                .filter_map(|s| match s {
                    ExportSpecifier::Named(n) => {
                        let orig_atom = match &n.orig {
                            ModuleExportName::Ident(id) => &id.sym,
                            ModuleExportName::Str(_) => return None,
                        };
                        let is_direct = match &n.exported {
                            None => true,
                            Some(ModuleExportName::Ident(id)) => id.sym == *orig_atom,
                            Some(ModuleExportName::Str(_)) => false,
                        };
                        if is_direct {
                            Some(orig_atom.clone())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();

    // Restore consumed namespace decls + __export calls whose namespace
    // binding is still referenced by the remaining entry or by factory
    // modules.  Re-inserting them keeps the namespace object alive;
    // importing the individual bindings ensures the __export getters
    // resolve correctly.
    let mut restored_items: Vec<ModuleItem> = Vec::new();
    let mut factory_ns_exports: Vec<Atom> = Vec::new();
    let mut restored_namespace_bindings: HashSet<BindingId> = HashSet::new();
    for &(ns_idx, call_idx, boundary) in &consumed_ns {
        let entry_needs = entry_referenced.contains(&boundary.ns_binding);
        let factory_needs = factory_referenced.contains(&boundary.ns_binding);
        if !entry_needs && !factory_needs {
            continue;
        }
        restored_namespace_bindings.insert(boundary.ns_binding.clone());
        restored_items.push(
            source_slots[ns_idx]
                .take()
                .expect("ns_decl already consumed"),
        );
        let _ = source_slots[call_idx]
            .take()
            .expect("export_call already consumed");
        // Restored namespaces are entry-level compatibility objects. Emitting
        // direct getters here avoids pulling the bundler's `__export` helper
        // and its late runtime aliases into the synthetic entry module.
        //
        // Extracted scope modules already use the same direct namespace setup
        // through `make_namespace_export_code`.
        restored_items.extend(make_namespace_define_property_items(
            &boundary.ns_binding.0,
            &boundary.export_entries,
        ));
        // If a factory references this namespace but the entry doesn't
        // already export it via an ESM export declaration, synthesize one
        // so the factory's `import { ns_a } from "./entry.js"` resolves.
        if factory_needs && !entry_already_exports.contains(&boundary.ns_binding.0) {
            factory_ns_exports.push(boundary.ns_binding.0.clone());
        }
    }
    for index in &removable_export_helper_indices {
        let _ = source_slots[*index].take();
    }
    if !factory_ns_exports.is_empty() {
        factory_ns_exports.sort();
        restored_items.push(make_scope_export_stmt(&factory_ns_exports));
    }
    let mut entry_imports: HashMap<usize, Vec<BindingId>> = HashMap::new();
    for ref_binding in &entry_referenced {
        if restored_namespace_bindings.contains(ref_binding) {
            continue;
        }
        if let Some(&source_mi) = binding_to_module.get(ref_binding) {
            entry_imports
                .entry(source_mi)
                .or_default()
                .push(ref_binding.clone());
        } else if let Some((source_binding, source_mi)) = binding_module_by_atom.get(&ref_binding.0)
        {
            entry_imports
                .entry(*source_mi)
                .or_default()
                .push(source_binding.clone());
        }
    }

    let mut remaining: Vec<ModuleItem> = Vec::new();
    let mut entry_tail = restored_items;
    entry_tail.extend(remaining_indices.iter().map(|&i| {
        source_slots[i]
            .take()
            .expect("remaining item already consumed")
    }));
    let mut entry_import_renames: Vec<BindingRename> = Vec::new();
    let mut entry_reserved_atoms: HashSet<Atom> = entry_tail
        .iter()
        .flat_map(|item| {
            module_item_declared_binding_ids(item)
                .into_iter()
                .chain(module_item_import_binding_ids(item))
        })
        .map(|(atom, _)| atom)
        .collect();
    if !entry_imports.is_empty() {
        let mut import_sources: Vec<usize> = entry_imports.keys().copied().collect();
        import_sources.sort();
        for source_mi in import_sources {
            let bindings = entry_imports.get_mut(&source_mi).unwrap();
            bindings.sort_by(|a, b| a.0.cmp(&b.0));
            bindings.dedup();
            let mut names = Vec::new();
            for binding in bindings {
                let imported = binding.0.clone();
                let local = reserve_import_atom(&imported, &mut entry_reserved_atoms);
                if local != imported {
                    entry_import_renames.push(BindingRename {
                        old: binding.clone(),
                        new: local.clone(),
                    });
                }
                names.push((imported, local));
            }
            remaining.push(make_scope_import_stmt_with_aliases(
                &names,
                &metas[source_mi].filename,
            ));
        }
    }
    let mut entry_factory_imports: HashMap<String, Vec<BindingId>> = HashMap::new();
    for ref_binding in &entry_referenced {
        if let Some(source_filename) = factory_preassigned_bindings.get(ref_binding) {
            let source_filename = binding_to_filename
                .get(ref_binding)
                .unwrap_or(source_filename);
            if source_filename == "entry.js" {
                continue;
            }
            entry_factory_imports
                .entry(source_filename.clone())
                .or_default()
                .push(ref_binding.clone());
        }
    }
    let mut entry_factory_filenames: Vec<String> = entry_factory_imports.keys().cloned().collect();
    entry_factory_filenames.sort();
    for source_filename in entry_factory_filenames {
        let bindings = entry_factory_imports.get_mut(&source_filename).unwrap();
        bindings.sort_by(|a, b| a.0.cmp(&b.0));
        bindings.dedup();
        let mut names = Vec::new();
        for binding in bindings {
            let imported = binding.0.clone();
            let local = reserve_import_atom(&imported, &mut entry_reserved_atoms);
            if local != imported {
                entry_import_renames.push(BindingRename {
                    old: binding.clone(),
                    new: local.clone(),
                });
            }
            names.push((imported, local));
        }
        let rel_path = relative_import_path("entry.js", &source_filename);
        remaining.push(make_scope_import_stmt_with_aliases(&names, &rel_path));
    }
    rename_bindings(&mut entry_tail, &entry_import_renames);
    remaining.extend(entry_tail);
    drop(build_entry_enter);

    (
        modules,
        remaining,
        binding_to_filename,
        module_already_imports,
        module_local_atoms,
        module_referenced_atoms,
        scope_claimed_factory_bindings,
    )
}

struct ScopeHoistedBoundary {
    ns_atom: Atom,
    ns_binding: BindingId,
    ns_decl_index: usize,
    export_call_index: usize,
    export_entries: Vec<(Atom, BindingId)>,
    exported_bindings: HashSet<BindingId>,
}

/// Detect the `__export` helper: an arrow with 2 params whose body is a
/// single for-in loop (iterating over the second param).
fn detect_export_helper(items: &[ModuleItem]) -> Option<(usize, BindingId)> {
    for (index, item) in items.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else { continue };
            let Some(init) = &decl.init else { continue };
            if is_export_helper(init) {
                return Some((index, (bi.id.sym.clone(), bi.id.ctxt)));
            }
        }
    }
    None
}

/// Check if an expression matches the __export pattern:
///   (target, all) => { for (var name in all) defProp(...) }
fn is_export_helper(expr: &Expr) -> bool {
    let Expr::Arrow(arrow) = expr else {
        return false;
    };
    if arrow.params.len() != 2 {
        return false;
    }
    let BlockStmtOrExpr::BlockStmt(block) = &*arrow.body else {
        return false;
    };
    if block.stmts.len() != 1 {
        return false;
    }
    matches!(&block.stmts[0], Stmt::ForIn(ForInStmt { right, .. })
        if matches!(&**right, Expr::Ident(id) if same_param_ident(&arrow.params[1], &id.sym)))
}

fn same_param_ident(pat: &Pat, sym: &Atom) -> bool {
    matches!(pat, Pat::Ident(bi) if bi.id.sym == *sym)
}

/// Find all namespace + __export call pairs.
/// Pattern: `var NS = {};` at index i, `__export(NS, { ... })` at index i+1.
fn collect_scope_hoisted_boundaries(
    items: &[ModuleItem],
    export_helper: &BindingId,
) -> Vec<ScopeHoistedBoundary> {
    let mut boundaries = Vec::new();

    for i in 0..items.len().saturating_sub(1) {
        // Check: var NS = {};
        let Some(ns_binding) = extract_empty_object_decl(&items[i]) else {
            continue;
        };

        // Check: __export(NS, { ... }) at i+1
        if !is_export_call(&items[i + 1], export_helper, &ns_binding) {
            continue;
        }

        let export_entries = extract_export_entries(&items[i + 1]);
        let exported_bindings = export_entries
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect();

        boundaries.push(ScopeHoistedBoundary {
            ns_atom: ns_binding.0.clone(),
            ns_binding,
            ns_decl_index: i,
            export_call_index: i + 1,
            export_entries,
            exported_bindings,
        });
    }

    boundaries
}

/// Check if a namespace atom appears in any ESM export declaration.
/// e.g. `export { math_exports as math }` contains the ident `math_exports`.
fn namespace_is_module_exported(
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    ns_binding: &BindingId,
) -> bool {
    items.iter().enumerate().any(|(i, item)| {
        matches!(item, ModuleItem::ModuleDecl(_))
            && item_infos
                .get(i)
                .is_some_and(|info| info.references.contains(ns_binding))
    })
}

/// Extract the binding atoms from `__export(NS, { key: () => binding, ... })`.
fn extract_export_entries(item: &ModuleItem) -> Vec<(Atom, BindingId)> {
    let mut entries = Vec::new();
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return entries;
    };
    let Expr::Call(call) = &**expr else {
        return entries;
    };
    if call.args.len() != 2 {
        return entries;
    }
    let Expr::Object(obj) = &*call.args[1].expr else {
        return entries;
    };
    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = &**prop else {
            continue;
        };
        let Some(export_name) = prop_name_atom(&kv.key) else {
            continue;
        };
        // Value is `() => binding` — extract the binding ident from the arrow body.
        let Expr::Arrow(arrow) = &*kv.value else {
            continue;
        };
        if let BlockStmtOrExpr::Expr(body_expr) = &*arrow.body {
            if let Expr::Ident(id) = &**body_expr {
                entries.push((export_name, (id.sym.clone(), id.ctxt)));
            }
        }
    }
    entries
}

fn prop_name_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(id) => Some(id.sym.clone()),
        PropName::Str(s) => Some(s.value.as_str().unwrap_or("").to_string().into()),
        _ => None,
    }
}

/// Extract the binding from `var X = {};` (single declarator, empty object init).
fn extract_empty_object_decl(item: &ModuleItem) -> Option<BindingId> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(bi) = &decl.name else {
        return None;
    };
    let Some(init) = &decl.init else {
        return None;
    };
    let Expr::Object(ObjectLit { props, .. }) = &**init else {
        return None;
    };
    if !props.is_empty() {
        return None;
    }
    Some((bi.id.sym.clone(), bi.id.ctxt))
}

/// Check if an item is `__export(NS, { ... })`.
fn is_export_call(item: &ModuleItem, export_helper: &BindingId, ns_binding: &BindingId) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return false;
    };
    let Expr::Call(call) = &**expr else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(callee_id) = &**callee else {
        return false;
    };
    if callee_id.sym != export_helper.0 || callee_id.ctxt != export_helper.1 || call.args.len() != 2
    {
        return false;
    }
    // First arg must be the namespace ident.
    let Expr::Ident(first_arg) = &*call.args[0].expr else {
        return false;
    };
    if first_arg.sym != ns_binding.0 || first_arg.ctxt != ns_binding.1 {
        return false;
    }
    // Second arg must be an object literal (the export map).
    matches!(&*call.args[1].expr, Expr::Object(_))
}

/// Find the end index for the last scope-hoisted module.
///
/// Three-phase scan from `from`:
///   Phase 1: find the last item that declares an exported binding.
///            Everything up to it (inclusive) is module code — this
///            captures private helpers that precede exported declarations.
///   Phase 2: reference closure — extend to include declarations of names
///            referenced by the module code (private helpers after exports).
///   Phase 3: include trailing expression statements that reference module
///            bindings (side effects). Stop at unreferenced expressions,
///            declarations, or ModuleDecls.
fn find_last_module_end(
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    from: usize,
    exported_bindings: &HashSet<BindingId>,
    factory_referenced_atoms: &HashSet<Atom>,
) -> usize {
    // Phase 1: find the last item that declares an exported binding.
    let mut last_export_idx = None;
    for (i, item) in items.iter().enumerate().skip(from) {
        if is_module_boundary_item(item) {
            break;
        }
        if item_infos[i]
            .declared
            .iter()
            .any(|binding| exported_bindings.contains(binding))
        {
            last_export_idx = Some(i);
        }
    }

    let Some(last) = last_export_idx else {
        return from;
    };

    // Phase 2: reference closure — include declarations whose names are
    // referenced by the module code collected so far OR by factory modules.
    // This captures private helpers that esbuild emits after the exported
    // functions, whether they are called by other scope-hoisted code or by
    // factory modules.
    let mut end = last + 1;
    let mut module_bindings: HashSet<BindingId> = exported_bindings.clone();
    while end < items.len() {
        let item = &items[end];
        if is_module_boundary_item(item) {
            break;
        }
        let declared = &item_infos[end].declared;
        if declared.is_empty() {
            break;
        };
        let referenced_by_module = declared
            .iter()
            .any(|binding| items_reference_binding(&item_infos[from..end], binding));
        let referenced_by_factory = declared
            .iter()
            .any(|(atom, _)| factory_referenced_atoms.contains(atom));
        if !referenced_by_module && !referenced_by_factory {
            break;
        }

        for binding in declared {
            module_bindings.insert(binding.clone());
        }
        end += 1;
    }

    // Phase 3: include trailing expression statements that reference any
    // binding from this module (side effects like `register("self", ...)`
    // or `console.log(value)`). Stop at expressions that only reference
    // globals/literals, declarations, or ModuleDecls.
    for (i, item) in items.iter().enumerate().skip(end) {
        match item {
            item if is_module_boundary_item(item) => return i,
            ModuleItem::Stmt(Stmt::Expr(_)) => {
                if !item_infos[i]
                    .references
                    .iter()
                    .any(|binding| module_bindings.contains(binding))
                {
                    return i;
                }
            }
            ModuleItem::Stmt(Stmt::Decl(_)) | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(_)) => {
                return i;
            }
            _ => return i,
        }
    }
    items.len()
}

fn is_module_boundary_item(item: &ModuleItem) -> bool {
    // `export var/function/class ...` can still belong to the current
    // scope-hoisted module; imports and re-export declarations start a
    // separate module boundary.
    matches!(item, ModuleItem::ModuleDecl(decl) if !matches!(decl, ModuleDecl::ExportDecl(_)))
}

fn items_reference_binding(item_infos: &[ItemBindingInfo], binding: &BindingId) -> bool {
    item_infos
        .iter()
        .any(|info| info.references.contains(binding))
}

fn removable_export_helper_dependency_indices(
    export_helper_index: usize,
    items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    boundaries: &[ScopeHoistedBoundary],
) -> HashSet<usize> {
    let mut binding_to_index = HashMap::new();
    for (index, info) in item_infos.iter().enumerate() {
        for binding in &info.declared {
            binding_to_index.entry(binding.clone()).or_insert(index);
        }
    }

    let mut closure = HashSet::new();
    let mut stack = vec![export_helper_index];
    while let Some(index) = stack.pop() {
        if !closure.insert(index) {
            continue;
        }
        for reference in &item_infos[index].references {
            if let Some(&decl_index) = binding_to_index.get(reference) {
                stack.push(decl_index);
            }
        }
    }

    let mut ignored_consumers = closure.clone();
    ignored_consumers.extend(boundaries.iter().map(|boundary| boundary.export_call_index));

    closure
        .into_iter()
        .filter(|&index| {
            is_removable_export_helper_dependency_item(&items[index])
                && item_infos[index].declared.iter().all(|binding| {
                    !item_infos.iter().enumerate().any(|(consumer_index, info)| {
                        !ignored_consumers.contains(&consumer_index)
                            && info.references.contains(binding)
                    })
                })
        })
        .collect()
}

fn is_removable_export_helper_dependency_item(item: &ModuleItem) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_))) => true,
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => var
            .decls
            .iter()
            .all(is_removable_export_helper_dependency_var),
        _ => false,
    }
}

fn is_removable_export_helper_dependency_var(decl: &VarDeclarator) -> bool {
    let Some(init) = decl.init.as_deref() else {
        return true;
    };

    matches!(init, Expr::Fn(_) | Expr::Arrow(_))
        || is_object_destructure_from_object(&decl.name, init)
        || is_object_member_alias(init)
}

fn is_scope_support_declaration_for_binding(item: &ModuleItem, binding: &BindingId) -> bool {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
            fn_decl.ident.sym == binding.0 && fn_decl.ident.ctxt == binding.1
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => var_decl.decls.iter().any(|decl| {
            pat_declared_binding_ids(&decl.name)
                .iter()
                .any(|decl_binding| decl_binding == binding)
                && decl
                    .init
                    .as_deref()
                    .is_some_and(|init| matches!(init, Expr::Fn(_) | Expr::Arrow(_)))
        }),
        _ => false,
    }
}

fn is_object_destructure_from_object(name: &Pat, init: &Expr) -> bool {
    matches!(name, Pat::Object(_)) && matches!(init, Expr::Ident(ident) if ident.sym == *"Object")
}

fn is_object_member_alias(init: &Expr) -> bool {
    let Expr::Member(member) = init else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(ident) if ident.sym == *"Object")
        && matches!(&member.prop, MemberProp::Ident(_))
}

#[derive(Default)]
struct ItemBindingInfo {
    declared: HashSet<BindingId>,
    references: HashSet<BindingId>,
}

fn build_item_binding_infos(items: &[ModuleItem]) -> Vec<ItemBindingInfo> {
    // Collect per-item declared bindings in one pass, then build the
    // union for reference filtering.  This avoids calling
    // module_item_declared_binding_ids twice per item.
    let per_item_declared: Vec<HashSet<BindingId>> = items
        .iter()
        .map(|item| module_item_declared_binding_ids(item).into_iter().collect())
        .collect();

    let top_level_bindings: HashSet<BindingId> = per_item_declared
        .iter()
        .flat_map(|s| s.iter().cloned())
        .chain(
            items
                .iter()
                .flat_map(|item| module_item_import_binding_ids(item).into_iter()),
        )
        .collect();

    items
        .iter()
        .zip(per_item_declared)
        .map(|(item, declared)| {
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &top_level_bindings,
                references: HashSet::new(),
            };
            item.visit_with(&mut collector);
            ItemBindingInfo {
                declared,
                references: collector.references,
            }
        })
        .collect()
}

fn item_binding_info_for(
    item: &ModuleItem,
    top_level_bindings: &HashSet<BindingId>,
) -> ItemBindingInfo {
    let declared: HashSet<BindingId> = module_item_declared_binding_ids(item).into_iter().collect();
    let mut collector = TopLevelRefCollector {
        top_level_bindings,
        references: HashSet::new(),
    };
    item.visit_with(&mut collector);
    ItemBindingInfo {
        declared,
        references: collector.references,
    }
}

fn module_item_import_binding_ids(item: &ModuleItem) -> Vec<BindingId> {
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
        return vec![];
    };
    import
        .specifiers
        .iter()
        .map(|specifier| match specifier {
            ImportSpecifier::Named(named) => (named.local.sym.clone(), named.local.ctxt),
            ImportSpecifier::Default(default) => (default.local.sym.clone(), default.local.ctxt),
            ImportSpecifier::Namespace(namespace) => {
                (namespace.local.sym.clone(), namespace.local.ctxt)
            }
        })
        .collect()
}

fn filter_item_excluding_bindings(
    item: &ModuleItem,
    excluded: &HashSet<BindingId>,
    excluded_atoms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let mut filtered = var_decl.clone();
            filtered.decls.retain(|decl| {
                let ids = pat_declared_binding_ids(&decl.name);
                ids.is_empty()
                    || !ids
                        .iter()
                        .all(|id| excluded.contains(id) || excluded_atoms.contains(&id.0))
            });
            if filtered.decls.is_empty() {
                None
            } else {
                Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))))
            }
        }
        _ => {
            let declared = module_item_declared_binding_ids(item);
            if !declared.is_empty()
                && declared
                    .iter()
                    .all(|id| excluded.contains(id) || excluded_atoms.contains(&id.0))
            {
                None
            } else {
                Some(item.clone())
            }
        }
    }
}

#[derive(Clone)]
struct ExternalImport {
    decl: ImportDecl,
    specifier: ImportSpecifier,
}

fn collect_external_imports(
    analysis_items: &[ModuleItem],
    source_items: &[ModuleItem],
) -> HashMap<BindingId, ExternalImport> {
    let mut imports = HashMap::new();
    for (analysis_item, source_item) in analysis_items.iter().zip(source_items) {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(analysis_import)) = analysis_item else {
            continue;
        };
        let ModuleItem::ModuleDecl(ModuleDecl::Import(source_import)) = source_item else {
            continue;
        };
        for (analysis_specifier, source_specifier) in analysis_import
            .specifiers
            .iter()
            .zip(source_import.specifiers.iter())
        {
            let binding = import_specifier_binding(analysis_specifier);
            imports.entry(binding).or_insert_with(|| ExternalImport {
                decl: source_import.clone(),
                specifier: source_specifier.clone(),
            });
        }
    }
    imports
}

fn import_specifier_binding(specifier: &ImportSpecifier) -> BindingId {
    match specifier {
        ImportSpecifier::Named(named) => (named.local.sym.clone(), named.local.ctxt),
        ImportSpecifier::Default(default) => (default.local.sym.clone(), default.local.ctxt),
        ImportSpecifier::Namespace(namespace) => {
            (namespace.local.sym.clone(), namespace.local.ctxt)
        }
    }
}

struct TopLevelRefCollector<'a> {
    top_level_bindings: &'a HashSet<BindingId>,
    references: HashSet<BindingId>,
}

struct AtomRefCollector<'a> {
    candidate_atoms: &'a HashSet<Atom>,
    references: HashSet<Atom>,
    shadowed_atoms: Vec<HashSet<Atom>>,
}

impl Visit for AtomRefCollector<'_> {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        if let Some(scope) = self.shadowed_atoms.last_mut() {
            scope.insert(ident.id.sym.clone());
        }
    }

    fn visit_function(&mut self, function: &Function) {
        self.shadowed_atoms.push(HashSet::new());
        function.visit_children_with(self);
        self.shadowed_atoms.pop();
    }

    fn visit_arrow_expr(&mut self, expr: &ArrowExpr) {
        self.shadowed_atoms.push(HashSet::new());
        expr.visit_children_with(self);
        self.shadowed_atoms.pop();
    }

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        if self.candidate_atoms.contains(&ident.sym)
            && !self
                .shadowed_atoms
                .iter()
                .any(|scope| scope.contains(&ident.sym))
        {
            self.references.insert(ident.sym.clone());
        }
    }

    fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
        expr.obj.visit_with(self);
        if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &swc_core::ecma::ast::MemberProp) {
        if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

impl TopLevelRefCollector<'_> {
    fn visit_binding_pat_defaults(&mut self, pat: &Pat) {
        match pat {
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.visit_binding_pat_defaults(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                            self.visit_binding_pat_defaults(&kv.value);
                        }
                        swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                            if let Some(value) = &assign.value {
                                value.visit_with(self);
                            }
                        }
                        swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                            self.visit_binding_pat_defaults(&rest.arg);
                        }
                    }
                }
            }
            Pat::Assign(assign) => {
                assign.right.visit_with(self);
                self.visit_binding_pat_defaults(&assign.left);
            }
            Pat::Rest(rest) => self.visit_binding_pat_defaults(&rest.arg),
            _ => {}
        }
    }
}

impl Visit for TopLevelRefCollector<'_> {
    fn visit_binding_ident(&mut self, _: &BindingIdent) {}

    fn visit_pat(&mut self, pat: &Pat) {
        self.visit_binding_pat_defaults(pat);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.visit_binding_pat_defaults(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        decl.function.visit_with(self);
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        decl.class.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        if let Some(ident) = assign.left.as_ident() {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.references.insert(binding);
            }
        }
        assign.left.visit_with(self);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = &*update.arg {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.references.insert(binding);
            }
        }
    }

    fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
        let binding = (ident.sym.clone(), ident.ctxt);
        if self.top_level_bindings.contains(&binding) {
            self.references.insert(binding);
        }
    }

    fn visit_member_expr(&mut self, expr: &swc_core::ecma::ast::MemberExpr) {
        expr.obj.visit_with(self);
        if let swc_core::ecma::ast::MemberProp::Computed(c) = &expr.prop {
            c.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &swc_core::ecma::ast::MemberProp) {
        if let swc_core::ecma::ast::MemberProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }

    fn visit_prop_name(&mut self, name: &PropName) {
        if let PropName::Computed(c) = name {
            c.visit_with(self);
        }
    }

    fn visit_super_prop(&mut self, prop: &swc_core::ecma::ast::SuperProp) {
        if let swc_core::ecma::ast::SuperProp::Computed(c) = prop {
            c.visit_with(self);
        }
    }
}

/// Collect top-level bindings that appear as assignment targets in a statement.
/// This detects `X = expr` and `X = expr, Y = expr` patterns where X/Y are
/// top-level bindings (not local declarations).
fn collect_write_bindings(
    stmt: &Stmt,
    top_level_bindings: &HashSet<BindingId>,
    out: &mut HashSet<BindingId>,
) {
    struct WriteCollector<'a> {
        top_level_bindings: &'a HashSet<BindingId>,
        writes: &'a mut HashSet<BindingId>,
    }

    impl Visit for WriteCollector<'_> {
        fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
            if let Some(ident) = assign.left.as_ident() {
                let binding = (ident.sym.clone(), ident.ctxt);
                if self.top_level_bindings.contains(&binding) {
                    self.writes.insert(binding);
                }
            } else if let swc_core::ecma::ast::AssignTarget::Pat(pat) = &assign.left {
                let mut collector = AssignTargetWriteCollector {
                    top_level_bindings: self.top_level_bindings,
                    writes: self.writes,
                };
                pat.visit_with(&mut collector);
            }
            assign.right.visit_with(self);
        }

        fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
            if let Expr::Ident(ident) = &*update.arg {
                let binding = (ident.sym.clone(), ident.ctxt);
                if self.top_level_bindings.contains(&binding) {
                    self.writes.insert(binding);
                }
            }
        }
    }

    struct AssignTargetWriteCollector<'a> {
        top_level_bindings: &'a HashSet<BindingId>,
        writes: &'a mut HashSet<BindingId>,
    }

    impl Visit for AssignTargetWriteCollector<'_> {
        fn visit_ident(&mut self, ident: &Ident) {
            let binding = (ident.sym.clone(), ident.ctxt);
            if self.top_level_bindings.contains(&binding) {
                self.writes.insert(binding);
            }
        }
    }

    let mut collector = WriteCollector {
        top_level_bindings,
        writes: out,
    };
    stmt.visit_with(&mut collector);
}

fn scope_write_atoms_for_item(
    item: &ModuleItem,
    top_level_bindings: &HashSet<BindingId>,
    top_level_atoms: &HashSet<Atom>,
) -> HashSet<Atom> {
    let mut local_collector = NonTopLevelBindingCollector {
        top_level_bindings,
        local_bindings: HashSet::new(),
    };
    item.visit_with(&mut local_collector);

    let mut write_collector = ScopeWriteAtomCollector {
        top_level_bindings,
        top_level_atoms,
        local_bindings: &local_collector.local_bindings,
        writes: HashSet::new(),
    };
    item.visit_with(&mut write_collector);
    write_collector.writes
}

struct NonTopLevelBindingCollector<'a> {
    top_level_bindings: &'a HashSet<BindingId>,
    local_bindings: HashSet<BindingId>,
}

impl Visit for NonTopLevelBindingCollector<'_> {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        let binding = (ident.id.sym.clone(), ident.id.ctxt);
        if !self.top_level_bindings.contains(&binding) {
            self.local_bindings.insert(binding);
        }
    }
}

struct ScopeWriteAtomCollector<'a> {
    top_level_bindings: &'a HashSet<BindingId>,
    top_level_atoms: &'a HashSet<Atom>,
    local_bindings: &'a HashSet<BindingId>,
    writes: HashSet<Atom>,
}

impl ScopeWriteAtomCollector<'_> {
    fn record_ident(&mut self, ident: &Ident) {
        let binding = (ident.sym.clone(), ident.ctxt);
        if self.top_level_bindings.contains(&binding)
            || (self.top_level_atoms.contains(&ident.sym)
                && !self.local_bindings.contains(&binding))
        {
            self.writes.insert(ident.sym.clone());
        }
    }
}

impl Visit for ScopeWriteAtomCollector<'_> {
    fn visit_assign_expr(&mut self, assign: &swc_core::ecma::ast::AssignExpr) {
        collect_scope_write_target(&assign.left, self);
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        if let Expr::Ident(ident) = update.arg.as_ref() {
            self.record_ident(ident);
        }
    }
}

fn collect_scope_write_target(target: &AssignTarget, collector: &mut ScopeWriteAtomCollector<'_>) {
    match target {
        AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) => {
            collector.record_ident(&ident.id);
        }
        AssignTarget::Simple(simple) => {
            simple.visit_with(collector);
        }
        AssignTarget::Pat(pat) => collect_scope_write_pat_target(pat, collector),
    }
}

fn collect_scope_write_pat_target(
    target: &AssignTargetPat,
    collector: &mut ScopeWriteAtomCollector<'_>,
) {
    match target {
        AssignTargetPat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_scope_write_pat(elem, collector);
            }
        }
        AssignTargetPat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_scope_write_pat(&kv.value, collector),
                    ObjectPatProp::Assign(assign) => collector.record_ident(&assign.key),
                    ObjectPatProp::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
                }
            }
        }
        AssignTargetPat::Invalid(_) => {}
    }
}

fn collect_scope_write_pat(pat: &Pat, collector: &mut ScopeWriteAtomCollector<'_>) {
    match pat {
        Pat::Ident(ident) => collector.record_ident(&ident.id),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_scope_write_pat(elem, collector);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::Assign(assign) => collector.record_ident(&assign.key),
                    ObjectPatProp::KeyValue(kv) => collect_scope_write_pat(&kv.value, collector),
                    ObjectPatProp::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
                }
            }
        }
        Pat::Rest(rest) => collect_scope_write_pat(&rest.arg, collector),
        Pat::Assign(assign) => {
            collect_scope_write_pat(&assign.left, collector);
            assign.right.visit_with(collector);
        }
        Pat::Expr(expr) => expr.visit_with(collector),
        Pat::Invalid(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Import / export synthesis for scope-hoisted modules
// ---------------------------------------------------------------------------

/// Compute a relative import specifier from `importer` to `target`.
/// Both are flat output filenames (e.g. `src/consumer.js`, `ns_a.js`).
/// Returns a string suitable for an ES import source (e.g. `./ns_a.js`,
/// `../ns_a.js`).
fn relative_import_path(importer: &str, target: &str) -> String {
    relative_import_specifier(importer, target)
}

fn reserve_import_atom(imported: &Atom, reserved: &mut HashSet<Atom>) -> Atom {
    if reserved.insert(imported.clone()) {
        return imported.clone();
    }

    for suffix in 2.. {
        let candidate: Atom = format!("{imported}${suffix}").into();
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
    }

    unreachable!("open-ended suffix search must find an unused import atom")
}

fn make_scope_import_stmt(names: &[Atom], from: &str) -> ModuleItem {
    let names: Vec<(Atom, Atom)> = names
        .iter()
        .map(|name| (name.clone(), name.clone()))
        .collect();
    make_scope_import_stmt_with_aliases(&names, from)
}

fn make_scope_import_stmt_with_aliases(names: &[(Atom, Atom)], from: &str) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|(imported, local)| {
            ImportSpecifier::Named(ImportNamedSpecifier {
                span: Default::default(),
                local: Ident::new(local.clone(), Default::default(), Default::default()),
                imported: if imported == local {
                    None
                } else {
                    Some(ModuleExportName::Ident(Ident::new(
                        imported.clone(),
                        Default::default(),
                        Default::default(),
                    )))
                },
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
        span: Default::default(),
        specifiers,
        src: Box::new(Str {
            span: Default::default(),
            value: if from.starts_with('.') || from.starts_with('/') {
                from.into()
            } else {
                format!("./{from}").into()
            },
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    }))
}

fn make_external_import_stmt(import: &ExternalImport) -> ModuleItem {
    let mut decl = import.decl.clone();
    decl.specifiers = vec![import.specifier.clone()];
    ModuleItem::ModuleDecl(ModuleDecl::Import(decl))
}

fn make_scope_export_stmt(names: &[Atom]) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|name| {
            ExportSpecifier::Named(ExportNamedSpecifier {
                span: Default::default(),
                orig: ModuleExportName::Ident(Ident::new(
                    name.clone(),
                    Default::default(),
                    Default::default(),
                )),
                exported: None,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span: Default::default(),
        specifiers,
        src: None,
        type_only: false,
        with: None,
    }))
}

fn make_namespace_define_property_items(
    namespace: &Atom,
    entries: &[(Atom, BindingId)],
) -> Vec<ModuleItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for (export_name, (binding_name, _)) in entries {
        if !seen.insert(export_name.clone()) {
            continue;
        }
        items.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: DUMMY_SP,
            expr: Box::new(Expr::Call(CallExpr {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                callee: Callee::Expr(Box::new(Expr::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(Expr::Ident(Ident::new(
                        "Object".into(),
                        DUMMY_SP,
                        SyntaxContext::empty(),
                    ))),
                    prop: MemberProp::Ident(IdentName::new("defineProperty".into(), DUMMY_SP)),
                }))),
                args: vec![
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Ident(Ident::new(
                            namespace.clone(),
                            DUMMY_SP,
                            SyntaxContext::empty(),
                        ))),
                    },
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Lit(Lit::Str(Str {
                            span: DUMMY_SP,
                            value: export_name.clone().into(),
                            raw: None,
                        }))),
                    },
                    ExprOrSpread {
                        spread: None,
                        expr: Box::new(Expr::Object(ObjectLit {
                            span: DUMMY_SP,
                            props: vec![
                                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                                    key: PropName::Ident(IdentName::new(
                                        "enumerable".into(),
                                        DUMMY_SP,
                                    )),
                                    value: Box::new(Expr::Lit(Lit::Bool(Bool {
                                        span: DUMMY_SP,
                                        value: true,
                                    }))),
                                }))),
                                PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                                    key: PropName::Ident(IdentName::new("get".into(), DUMMY_SP)),
                                    value: Box::new(Expr::Arrow(ArrowExpr {
                                        span: DUMMY_SP,
                                        ctxt: SyntaxContext::empty(),
                                        params: Vec::new(),
                                        body: Box::new(BlockStmtOrExpr::Expr(Box::new(
                                            Expr::Ident(Ident::new(
                                                binding_name.clone(),
                                                DUMMY_SP,
                                                SyntaxContext::empty(),
                                            )),
                                        ))),
                                        is_async: false,
                                        is_generator: false,
                                        type_params: None,
                                        return_type: None,
                                    })),
                                }))),
                            ],
                        })),
                    },
                ],
                type_args: None,
            })),
        })));
    }
    items
}

fn make_namespace_export_code(namespace: &Atom, entries: &[(Atom, BindingId)]) -> String {
    let mut code = format!("export var {namespace} = {{}};\n");
    let mut seen = HashSet::new();
    for (export_name, (binding_name, _)) in entries {
        if !seen.insert(export_name.clone()) {
            continue;
        }
        code.push_str(&format!(
            "Object.defineProperty({namespace}, {}, {{ enumerable: true, get: () => {binding_name} }});\n",
            js_string_literal(export_name)
        ));
    }
    code
}

fn js_string_literal(value: &Atom) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn factory_owned_decl_items(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_items: &HashMap<BindingId, (usize, ModuleItem, ModuleItem)>,
) -> Vec<ModuleItem> {
    factory_owned_decl_items_from(filename, factory_owned_bindings, top_level_decl_items, true)
}

fn scope_owned_support_decl_items(
    owned: &HashSet<BindingId>,
    decl_index_by_binding: &HashMap<BindingId, usize>,
    source_items: &[ModuleItem],
) -> Vec<ModuleItem> {
    if owned.is_empty() {
        return vec![];
    }
    let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    let mut item_indices: Vec<(usize, ModuleItem)> = owned
        .iter()
        .filter_map(|binding| {
            decl_index_by_binding
                .get(binding)
                .map(|index| (*index, source_items[*index].clone()))
        })
        .collect();
    item_indices.sort_by_key(|(index, _)| *index);
    item_indices.dedup_by_key(|(index, _)| *index);
    item_indices
        .into_iter()
        .filter_map(|(_, item)| filter_item_to_owned_bindings(&item, &owned_atoms))
        .collect()
}

fn factory_owned_analysis_decl_items(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_items: &HashMap<BindingId, (usize, ModuleItem, ModuleItem)>,
) -> Vec<ModuleItem> {
    factory_owned_decl_items_from(
        filename,
        factory_owned_bindings,
        top_level_decl_items,
        false,
    )
}

fn factory_owned_decl_items_from(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_items: &HashMap<BindingId, (usize, ModuleItem, ModuleItem)>,
    use_source: bool,
) -> Vec<ModuleItem> {
    let Some(owned) = factory_owned_bindings.get(filename) else {
        return vec![];
    };
    let owned_atoms: HashSet<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    let mut item_indices: Vec<(usize, ModuleItem)> = owned
        .iter()
        .filter_map(|binding| {
            top_level_decl_items
                .get(binding)
                .map(|(index, source, analysis)| {
                    if use_source {
                        (*index, source.clone())
                    } else {
                        (*index, analysis.clone())
                    }
                })
        })
        .collect();
    item_indices.sort_by_key(|(index, _)| *index);
    item_indices.dedup_by_key(|(index, _)| *index);
    item_indices
        .into_iter()
        .filter_map(|(_, item)| filter_item_to_owned_bindings(&item, &owned_atoms))
        .collect()
}

fn factory_owned_export_items(
    filename: &str,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
) -> Vec<ModuleItem> {
    let Some(owned) = factory_owned_bindings.get(filename) else {
        return vec![];
    };
    let mut names: Vec<Atom> = owned.iter().map(|(atom, _)| atom.clone()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        vec![]
    } else {
        vec![make_scope_export_stmt(&names)]
    }
}

fn filter_item_to_owned_bindings(
    item: &ModuleItem,
    owned_atoms: &HashSet<Atom>,
) -> Option<ModuleItem> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
            if owned_atoms.contains(&fn_decl.ident.sym) =>
        {
            Some(item.clone())
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
            if owned_atoms.contains(&class_decl.ident.sym) =>
        {
            Some(item.clone())
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let stmt_bindings: HashSet<BindingId> = var_decl
                .decls
                .iter()
                .flat_map(|decl| pat_declared_binding_ids(&decl.name))
                .collect();
            let mut keep_atoms: HashSet<Atom> = HashSet::new();
            for decl in &var_decl.decls {
                let decl_atoms: Vec<Atom> = pat_declared_binding_ids(&decl.name)
                    .into_iter()
                    .map(|(atom, _)| atom)
                    .collect();
                if !decl_atoms.iter().any(|atom| owned_atoms.contains(atom)) {
                    continue;
                }
                keep_atoms.extend(decl_atoms);
                let mut collector = TopLevelRefCollector {
                    top_level_bindings: &stmt_bindings,
                    references: HashSet::new(),
                };
                decl.visit_with(&mut collector);
                keep_atoms.extend(collector.references.into_iter().map(|(atom, _)| atom));
            }

            if var_decl
                .decls
                .iter()
                .any(|decl| pat_declares_owned(&decl.name, &keep_atoms))
            {
                let mut filtered = var_decl.clone();
                filtered
                    .decls
                    .retain(|decl| pat_declares_owned(&decl.name, &keep_atoms));
                Some(ModuleItem::Stmt(Stmt::Decl(Decl::Var(filtered))))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn pat_declared_binding_ids(pat: &Pat) -> Vec<BindingId> {
    find_pat_ids(pat)
}

fn pat_declares_owned(pat: &Pat, owned_atoms: &HashSet<Atom>) -> bool {
    match pat {
        Pat::Ident(bi) => owned_atoms.contains(&bi.id.sym),
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| pat_declares_owned(elem, owned_atoms)),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                pat_declares_owned(&kv.value, owned_atoms)
            }
            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                owned_atoms.contains(&assign.key.sym)
            }
            swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                pat_declares_owned(&rest.arg, owned_atoms)
            }
        }),
        Pat::Assign(assign) => pat_declares_owned(&assign.left, owned_atoms),
        Pat::Rest(rest) => pat_declares_owned(&rest.arg, owned_atoms),
        _ => false,
    }
}

enum ScopeExportPromotion {
    Promoted(ModuleItem, Vec<Atom>),
    Unchanged(ModuleItem),
}

fn try_promote_scope_export(item: ModuleItem, exported: &HashSet<Atom>) -> ScopeExportPromotion {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(ref fn_decl)))
            if exported.contains(&fn_decl.ident.sym) =>
        {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item else {
                unreachable!()
            };
            let names = vec![fn_decl.ident.sym.clone()];
            ScopeExportPromotion::Promoted(
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span: Default::default(),
                    decl: Decl::Fn(fn_decl),
                })),
                names,
            )
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(ref class_decl)))
            if exported.contains(&class_decl.ident.sym) =>
        {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl))) = item else {
                unreachable!()
            };
            let names = vec![class_decl.ident.sym.clone()];
            ScopeExportPromotion::Promoted(
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span: Default::default(),
                    decl: Decl::Class(class_decl),
                })),
                names,
            )
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            if var_decl.decls.len() == 1 {
                let decl = &var_decl.decls[0];
                if let Pat::Ident(bi) = &decl.name {
                    if exported.contains(&bi.id.sym)
                        && decl.init.as_deref().is_some_and(is_noop_arrow_expr)
                    {
                        let names = vec![bi.id.sym.clone()];
                        return ScopeExportPromotion::Promoted(
                            make_noop_export_function(&bi.id.sym),
                            names,
                        );
                    }
                }
            }
            let all_exported = var_decl
                .decls
                .iter()
                .all(|d| matches!(&d.name, Pat::Ident(bi) if exported.contains(&bi.id.sym)));
            if !all_exported {
                return ScopeExportPromotion::Unchanged(ModuleItem::Stmt(Stmt::Decl(Decl::Var(
                    var_decl,
                ))));
            }
            let names: Vec<Atom> = var_decl
                .decls
                .iter()
                .filter_map(|d| {
                    if let Pat::Ident(bi) = &d.name {
                        Some(bi.id.sym.clone())
                    } else {
                        Option::None
                    }
                })
                .collect();
            if names.is_empty() {
                return ScopeExportPromotion::Unchanged(ModuleItem::Stmt(Stmt::Decl(Decl::Var(
                    var_decl,
                ))));
            }
            ScopeExportPromotion::Promoted(
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span: Default::default(),
                    decl: Decl::Var(var_decl),
                })),
                names,
            )
        }
        item => ScopeExportPromotion::Unchanged(item),
    }
}

fn is_noop_arrow_expr(expr: &Expr) -> bool {
    let Expr::Arrow(ArrowExpr { params, body, .. }) = expr else {
        return false;
    };
    params.is_empty()
        && matches!(
            &**body,
            BlockStmtOrExpr::BlockStmt(block) if block.stmts.is_empty()
        )
}

fn make_noop_export_function(name: &Atom) -> ModuleItem {
    ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
        span: Default::default(),
        decl: Decl::Fn(FnDecl {
            ident: Ident::new(name.clone(), Default::default(), Default::default()),
            declare: false,
            function: Box::new(Function {
                params: vec![],
                decorators: vec![],
                span: Default::default(),
                ctxt: Default::default(),
                body: Some(BlockStmt {
                    span: Default::default(),
                    ctxt: Default::default(),
                    stmts: vec![],
                }),
                is_generator: false,
                is_async: false,
                type_params: None,
                return_type: None,
            }),
        }),
    }))
}

/// Case-insensitive filename dedup matching the CLI's `deduplicate_path` logic.
/// Probes `filename`, then `{stem}_2.{ext}`, `{stem}_3.{ext}`, ... until a
/// name not in `seen` is found.  Inserts the winner and returns it.
fn dedup_filename(filename: &str, seen: &mut HashSet<String>) -> String {
    if seen.insert(filename.to_ascii_lowercase()) {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) => (&filename[..i], &filename[i + 1..]),
        None => (filename, "js"),
    };
    let mut n = 2u32;
    loop {
        let candidate = format!("{stem}_{n}.{ext}");
        if seen.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

fn emit_items(items: Vec<ModuleItem>, filename: String, cm: Lrc<SourceMap>) -> String {
    let span = tracing::info_span!("esbuild: emit_items", count = items.len());
    let _enter = span.enter();
    let module = Module {
        span: Default::default(),
        body: items,
        shebang: None,
    };
    emit_module(module, filename, cm)
}

// ---------------------------------------------------------------------------
// Code generation
// ---------------------------------------------------------------------------

fn emit_module(module: Module, filename: String, cm: Lrc<SourceMap>) -> String {
    let _fm = cm.new_source_file(FileName::Custom(filename).into(), String::new());
    emit_module_raw(&module, cm).unwrap_or_default()
}

fn emit_module_raw(module: &Module, cm: Lrc<SourceMap>) -> anyhow::Result<String> {
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut output, None),
        };
        emitter.emit_module(module)?;
    }
    String::from_utf8(output).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_atom_refs(source: &str, candidates: &[&str]) -> HashSet<Atom> {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let module = super::super::parse_es_module(source, "atom-ref-test.js", cm)
                .expect("test source should parse");
            let candidate_atoms: HashSet<Atom> =
                candidates.iter().map(|name| Atom::from(*name)).collect();
            let mut collector = AtomRefCollector {
                candidate_atoms: &candidate_atoms,
                references: HashSet::new(),
                shadowed_atoms: vec![HashSet::new()],
            };
            module.visit_with(&mut collector);
            collector.references
        })
    }

    #[test]
    fn atom_ref_collector_finds_unbound_candidate_refs() {
        let refs = collect_atom_refs(
            r#"
function read(q) {
    return JA(q);
}
var obj = { [JA]: true };
"#,
            &["JA"],
        );

        assert!(refs.contains(&Atom::from("JA")));
    }

    #[test]
    fn atom_ref_collector_skips_shadowed_refs_and_static_property_keys() {
        let refs = collect_atom_refs(
            r#"
function read(JA) {
    return JA;
}
var obj = { JA: true };
"#,
            &["JA"],
        );

        assert!(
            !refs.contains(&Atom::from("JA")),
            "parameter references and static object keys should not synthesize imports"
        );
    }

    #[test]
    fn import_augmentation_only_adds_specifiers_to_existing_sources() {
        let mut binding_to_filename = HashMap::new();
        binding_to_filename.insert((Atom::from("NT"), Default::default()), "NT.js".to_string());
        binding_to_filename.insert((Atom::from("JA"), Default::default()), "NT.js".to_string());
        binding_to_filename.insert(
            (Atom::from("Other"), Default::default()),
            "Other.js".to_string(),
        );
        let referenced_atoms = [Atom::from("JA"), Atom::from("Other")]
            .into_iter()
            .collect();
        let mut imports_by_source =
            HashMap::from([(String::from("NT.js"), vec![Atom::from("NT")])]);
        let binding_filename_by_atom = atom_to_filename_binding_map(&binding_to_filename);

        augment_imports_with_referenced_atoms_for_existing_sources(
            &mut imports_by_source,
            "D38_2.js",
            &referenced_atoms,
            &binding_filename_by_atom,
            None,
        );

        let nt_imports = imports_by_source.get("NT.js").unwrap();
        assert!(nt_imports.contains(&Atom::from("JA")));
        assert!(
            !imports_by_source.contains_key("Other.js"),
            "augmentation must not create new import edges"
        );
    }

    #[test]
    fn factory_atom_import_can_create_filename_edge() {
        let mut imports_by_filename = HashMap::new();
        let binding = (Atom::from("RT6"), Default::default());

        add_factory_atom_import(&mut imports_by_filename, "Zaq_2.js", &binding, "RT6.js");

        assert_eq!(
            imports_by_filename.get("RT6.js"),
            Some(&vec![binding.clone()])
        );

        add_factory_atom_import(&mut imports_by_filename, "RT6.js", &binding, "RT6.js");

        assert_eq!(
            imports_by_filename.get("RT6.js"),
            Some(&vec![binding]),
            "self imports should still be ignored"
        );
    }
}
