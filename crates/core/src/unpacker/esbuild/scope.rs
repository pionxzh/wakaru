//! Scope-hoisted module extraction from the entry items.

use swc_core::atoms::Atom;
use swc_core::common::sync::Lrc;
use swc_core::common::{SourceMap, Span, Spanned};
use swc_core::ecma::ast::{
    ArrowExpr, CallExpr, Callee, Decl, ExportDecl, ExportSpecifier, Expr, Function, Ident,
    MemberProp, ModuleDecl, ModuleExportName, ModuleItem, Pat, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::collections::{HashMap, HashSet};
use crate::rules::rename_utils::{rename_bindings, BindingRename};
use crate::unpacker::emit_esm::{make_named_export_stmt, make_named_import_stmt_with_aliases};
use crate::unpacker::{
    module_item_declared_binding_ids, spans_byte_ranges, BindingId, SourcePositions, UnpackedModule,
};
use crate::utils::paren::strip_parens;

use super::bindings::{
    add_factory_atom_import, atom_to_filename_binding_map, atom_to_module_binding_map,
    build_item_binding_infos, collect_external_imports, exact_write_bindings_for_item,
    filter_item_excluding_bindings, item_binding_info_for, module_item_import_binding_ids,
    scope_write_atoms_for_item, AtomRefCollector, ExternalImport, ItemBindingInfo,
};
use super::scope_boundaries::{
    collect_scope_hoisted_boundaries, detect_export_helper, find_last_module_end,
    is_scope_support_declaration_for_binding, removable_export_helper_dependency_indices,
    ScopeHoistedBoundary,
};
use super::synthesis::{
    dedup_filename, emit_items, filter_item_to_owned_bindings, make_external_import_stmt,
    make_namespace_define_property_items, make_namespace_object_decl, relative_import_path,
    reserve_import_atom, retain_owned_support_source_items, scope_owned_support_decl_items,
    try_promote_scope_export, ScopeExportPromotion,
};

/// Metadata collected during the first pass over scope-hoisted boundaries.
#[derive(Clone)]
struct ScopeNamespaceExport {
    namespace_binding: BindingId,
    pub(super) export_entries: Vec<(Atom, BindingId)>,
}

#[derive(Clone)]
struct ScopeModuleMeta {
    namespaces: Vec<ScopeNamespaceExport>,
    body_indices: Vec<usize>,
    owned_support_bindings: HashSet<BindingId>,
    pub(super) exported_bindings: HashSet<BindingId>,
    exported_atoms: HashSet<Atom>,
    pub(super) declared_bindings: HashSet<BindingId>,
    local_import_bindings: HashSet<BindingId>,
    pub(super) referenced_bindings: HashSet<BindingId>,
    written_atoms: HashSet<Atom>,
    pub(super) referenced_atoms: HashSet<Atom>,
    pub(super) filename: String,
    pub(super) id: String,
}

pub(super) struct ScopeExtractionRefs<'a> {
    pub(super) factory_referenced: &'a HashSet<BindingId>,
    pub(super) factory_preassigned_bindings: &'a HashMap<BindingId, String>,
    pub(super) factory_importable_bindings: &'a HashMap<BindingId, String>,
    pub(super) drop_unowned_helper_sibling_indices: &'a HashSet<usize>,
    pub(super) positions: SourcePositions,
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
    let mut writer_modules_by_factory: HashMap<String, Vec<usize>> = HashMap::default();
    for (mi, meta) in metas.iter().enumerate() {
        let mut factories = HashSet::default();
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

    let mut adjacency: Vec<HashSet<usize>> = vec![HashSet::default(); metas.len()];
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

    let mut group_by_first: HashMap<usize, Vec<usize>> = HashMap::default();
    let mut member_to_first: HashMap<usize, usize> = HashMap::default();
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
/// Everything `extract_scope_hoisted_modules` returns to the caller.
#[derive(Default)]
pub(super) struct ScopeExtractionResult {
    pub(super) modules: Vec<UnpackedModule>,
    pub(super) remaining_entry: Vec<ModuleItem>,
    pub(super) binding_to_filename: HashMap<BindingId, String>,
    pub(super) module_already_imports: HashMap<String, HashSet<BindingId>>,
    pub(super) module_local_atoms: HashMap<String, HashSet<Atom>>,
    pub(super) module_referenced_atoms: HashMap<String, HashSet<Atom>>,
    pub(super) scope_claimed_factory_bindings: HashMap<BindingId, String>,
}

/// Module-level facts collected before any item is moved out of the entry.
struct ScopeMetadata {
    pub(super) export_helper_index: usize,
    pub(super) item_infos: Vec<ItemBindingInfo>,
    pub(super) top_level_bindings: HashSet<BindingId>,
    pub(super) top_level_atoms: HashSet<Atom>,
    pub(super) external_imports: HashMap<BindingId, ExternalImport>,
    pub(super) boundaries: Vec<ScopeHoistedBoundary>,
}

/// Output of the partition pass: per-module metadata plus which entry items
/// each scope-hoisted module consumed.
struct ScopePartition<'b> {
    pub(super) source_slots: Vec<Option<ModuleItem>>,
    metas: Vec<ScopeModuleMeta>,
    consumed: HashSet<usize>,
    consumed_ns: Vec<(usize, usize, &'b ScopeHoistedBoundary)>,
    removable_export_helper_indices: HashSet<usize>,
    reference_candidate_atoms: HashSet<Atom>,
}

struct ScopeBindingMaps {
    factory_preassigned_by_atom: HashMap<Atom, (BindingId, String)>,
    binding_to_module: HashMap<BindingId, usize>,
    pub(super) decl_index_by_binding: HashMap<BindingId, usize>,
}

struct ScopeImportExportMaps {
    remaining_indices: Vec<usize>,
    entry_referenced: HashSet<BindingId>,
    /// Bindings that stay in the synthetic entry (remaining declarations and
    /// restorable namespace objects) but are referenced by a scope module.
    /// Each needs an `import ... from "./entry.js"` in the consumer and an
    /// `export` from the entry.
    scope_needed_entry_bindings: HashSet<BindingId>,
    /// Entry reads proven safe for each consumer, independently of siblings.
    scope_entry_imports: Vec<HashSet<BindingId>>,
    effective_exports: Vec<HashSet<Atom>>,
    pub(super) binding_to_filename: HashMap<BindingId, String>,
    pub(super) scope_claimed_factory_bindings: HashMap<BindingId, String>,
    factory_binding_filename_by_atom: HashMap<Atom, (BindingId, String)>,
    pub(super) binding_filename_by_atom: HashMap<Atom, (BindingId, String)>,
    filename_to_module: HashMap<String, usize>,
    binding_module_by_atom: HashMap<Atom, (BindingId, usize)>,
}

struct ScopeEmittedModules {
    pub(super) modules: Vec<UnpackedModule>,
    pub(super) module_local_atoms: HashMap<String, HashSet<Atom>>,
    pub(super) module_referenced_atoms: HashMap<String, HashSet<Atom>>,
}

pub(super) fn extract_scope_hoisted_modules(
    analysis_items: &[ModuleItem],
    source_items: Vec<ModuleItem>,
    seen_lower: &mut HashSet<String>,
    cm: Lrc<SourceMap>,
    refs: ScopeExtractionRefs<'_>,
) -> ScopeExtractionResult {
    debug_assert_eq!(analysis_items.len(), source_items.len());

    let Some(metadata) = collect_scope_metadata(analysis_items, &source_items) else {
        return ScopeExtractionResult {
            remaining_entry: source_items,
            ..Default::default()
        };
    };

    let mut partition =
        partition_scope_modules(analysis_items, source_items, seen_lower, &metadata, &refs);
    let mut maps = build_scope_binding_maps(&mut partition.metas, &metadata, &refs);
    let owned_support_source_items =
        adopt_scope_support_decls(analysis_items, &metadata, &mut partition, &mut maps, &refs);
    // Hoisted support declarations are adopted after the initial partition.
    // Their writes can reveal additional modules that must share ownership of
    // one lazy factory's mutable state, so repeat the conflict merge with the
    // complete write inventory and rebuild the module index.
    merge_conflicting_factory_scope_metas(&mut partition.metas, &maps.factory_preassigned_by_atom);
    maps.binding_to_module = scope_binding_to_module(&partition.metas);
    let ie = compute_scope_imports_exports(analysis_items, &metadata, &partition, &maps, &refs);
    let emitted = emit_scope_modules(
        cm,
        &metadata,
        &mut partition,
        &maps,
        &ie,
        &owned_support_source_items,
        &refs,
    );
    let (module_already_imports, remaining_entry) =
        build_scope_entry(&metadata, &mut partition, &maps, &ie, &refs);

    ScopeExtractionResult {
        modules: emitted.modules,
        remaining_entry,
        binding_to_filename: ie.binding_to_filename,
        module_already_imports,
        module_local_atoms: emitted.module_local_atoms,
        module_referenced_atoms: emitted.module_referenced_atoms,
        scope_claimed_factory_bindings: ie.scope_claimed_factory_bindings,
    }
}

fn collect_scope_metadata(
    analysis_items: &[ModuleItem],
    source_items: &[ModuleItem],
) -> Option<ScopeMetadata> {
    let span = tracing::info_span!("esbuild: scope collect metadata");
    let _enter = span.enter();

    // Step 1: find the __export helper binding.
    let (export_helper_index, export_helper) = detect_export_helper(analysis_items)?;
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
    let external_imports = collect_external_imports(analysis_items, source_items);

    // Step 2: find all (namespace_decl_index, export_call_index, ns_atom) triples.
    let boundaries = collect_scope_hoisted_boundaries(analysis_items, &export_helper);
    if boundaries.is_empty() {
        return None;
    }

    Some(ScopeMetadata {
        export_helper_index,
        item_infos,
        top_level_bindings,
        top_level_atoms,
        external_imports,
        boundaries,
    })
}

fn partition_scope_modules<'b>(
    analysis_items: &[ModuleItem],
    source_items: Vec<ModuleItem>,
    seen_lower: &mut HashSet<String>,
    metadata: &'b ScopeMetadata,
    refs: &ScopeExtractionRefs<'_>,
) -> ScopePartition<'b> {
    let export_helper_index = metadata.export_helper_index;
    let item_infos = &metadata.item_infos;
    let top_level_bindings = &metadata.top_level_bindings;
    let top_level_atoms = &metadata.top_level_atoms;
    let boundaries = &metadata.boundaries;
    let factory_referenced = refs.factory_referenced;
    let factory_preassigned_bindings = refs.factory_preassigned_bindings;
    let factory_importable_bindings = refs.factory_importable_bindings;

    // Convert to Option<ModuleItem> so items can be moved out by index.
    let mut source_slots: Vec<Option<ModuleItem>> = source_items.into_iter().map(Some).collect();

    // Step 3 (pass 1): partition items and collect per-module metadata.
    let span = tracing::info_span!("esbuild: scope partition modules", count = boundaries.len());
    let _enter = span.enter();
    let mut metas: Vec<ScopeModuleMeta> = Vec::new();
    let mut consumed: HashSet<usize> = HashSet::default();
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
        item_infos,
        boundaries,
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
                item_infos,
                boundary.export_call_index + 1,
                &boundary.exported_bindings,
                &factory_referenced_atoms,
            )
        };

        let mut body_indices: Vec<usize> = Vec::new();
        let mut declared_bindings: HashSet<BindingId> = HashSet::default();
        let mut local_import_bindings: HashSet<BindingId> = HashSet::default();
        let mut referenced_bindings: HashSet<BindingId> = HashSet::default();
        let mut written_atoms: HashSet<Atom> = HashSet::default();
        let mut referenced_atoms: HashSet<Atom> = HashSet::default();

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
                filtered_info = Some(item_binding_info_for(&item, top_level_bindings));
                filtered_analysis_item = Some(item);
            }
            let analysis_item_for_visits = filtered_analysis_item
                .as_ref()
                .unwrap_or(&analysis_items[i]);
            let info = filtered_info.as_ref().unwrap_or(&item_infos[i]);

            let mut atom_collector = AtomRefCollector {
                candidate_atoms: &reference_candidate_atoms,
                references: HashSet::default(),
                shadowed_atoms: vec![HashSet::default()],
            };
            analysis_item_for_visits.visit_with(&mut atom_collector);
            body_indices.push(i);
            declared_bindings.extend(info.declared.iter().cloned());
            local_import_bindings.extend(module_item_import_binding_ids(analysis_item_for_visits));
            referenced_bindings.extend(info.references.iter().cloned());
            written_atoms.extend(scope_write_atoms_for_item(
                analysis_item_for_visits,
                top_level_bindings,
                top_level_atoms,
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
            owned_support_bindings: HashSet::default(),
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

    ScopePartition {
        source_slots,
        metas,
        consumed,
        consumed_ns,
        removable_export_helper_indices,
        reference_candidate_atoms,
    }
}

fn build_scope_binding_maps(
    metas: &mut Vec<ScopeModuleMeta>,
    metadata: &ScopeMetadata,
    refs: &ScopeExtractionRefs<'_>,
) -> ScopeBindingMaps {
    let span = tracing::info_span!("esbuild: scope build binding maps", count = metas.len());
    let _enter = span.enter();
    let item_infos = &metadata.item_infos;
    let factory_preassigned_bindings = refs.factory_preassigned_bindings;

    let factory_preassigned_by_atom = atom_to_filename_binding_map(factory_preassigned_bindings);
    merge_conflicting_factory_scope_metas(metas, &factory_preassigned_by_atom);

    // Build binding → module index map for all scope-hoisted modules.
    let binding_to_module = scope_binding_to_module(metas);

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

    ScopeBindingMaps {
        factory_preassigned_by_atom,
        binding_to_module,
        decl_index_by_binding,
    }
}

fn scope_binding_to_module(metas: &[ScopeModuleMeta]) -> HashMap<BindingId, usize> {
    let mut binding_to_module = HashMap::default();
    for (mi, meta) in metas.iter().enumerate() {
        for namespace in &meta.namespaces {
            binding_to_module.insert(namespace.namespace_binding.clone(), mi);
        }
        for binding in &meta.declared_bindings {
            binding_to_module.insert(binding.clone(), mi);
        }
    }
    binding_to_module
}

fn adopt_scope_support_decls(
    analysis_items: &[ModuleItem],
    metadata: &ScopeMetadata,
    partition: &mut ScopePartition<'_>,
    maps: &mut ScopeBindingMaps,
    refs: &ScopeExtractionRefs<'_>,
) -> HashMap<usize, ModuleItem> {
    // Scope-hoisted modules often call small top-level helpers that sit before
    // the namespace block. Move safe helper-like declarations into the first
    // extracted module that needs them so generated modules don't reference
    // invisible bindings left behind in entry.js.
    let span = tracing::info_span!("esbuild: scope adopt support decls");
    let _enter = span.enter();
    let ScopePartition {
        source_slots,
        metas,
        consumed,
        reference_candidate_atoms,
        ..
    } = partition;
    let item_infos = &metadata.item_infos;
    let external_imports = &metadata.external_imports;
    let binding_to_module = &mut maps.binding_to_module;
    let decl_index_by_binding = &maps.decl_index_by_binding;
    let factory_preassigned_bindings = refs.factory_preassigned_bindings;
    let factory_importable_bindings = refs.factory_importable_bindings;
    let drop_unowned_helper_sibling_indices = refs.drop_unowned_helper_sibling_indices;

    // A hoisted entry function can write extracted state without being called
    // by that state's module. Adopt it by its writes, not only by references
    // from the owner. Moving a function declaration has no eager initializer;
    // require all state writes to have one owner. Entry-only callers can keep
    // read-only entry dependencies through deferred imports; functions reached
    // from extracted modules must already have every dependency in their owner.
    // Exporting a function that is itself reassigned would merely move the
    // import-write error from its state binding to its own callable binding.
    let reassigned_bindings: HashSet<BindingId> = analysis_items
        .iter()
        .flat_map(|item| exact_write_bindings_for_item(item, &metadata.top_level_bindings))
        .collect();
    let scope_referenced: HashSet<&BindingId> = metas
        .iter()
        .flat_map(|meta| {
            meta.referenced_bindings
                .iter()
                .chain(&meta.exported_bindings)
        })
        .collect();
    let mut entry_writers: HashMap<usize, HashSet<BindingId>> = HashMap::default();
    for (index, item) in analysis_items.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = item else {
            continue;
        };
        let binding = function.ident.to_id();
        if consumed.contains(&index)
            || reassigned_bindings.contains(&binding)
            || binding_to_module.contains_key(&binding)
            || factory_preassigned_bindings.contains_key(&binding)
            || factory_importable_bindings.contains_key(&binding)
        {
            continue;
        }
        let writes = exact_write_bindings_for_item(item, &metadata.top_level_bindings);
        let Some(owner) = writes
            .iter()
            .next()
            .and_then(|id| binding_to_module.get(id))
            .copied()
        else {
            continue;
        };
        let entry_only =
            !scope_referenced.contains(&binding) && !refs.factory_referenced.contains(&binding);
        if !writes
            .iter()
            .all(|id| binding_to_module.get(id) == Some(&owner))
            || !item_infos[index].references.iter().all(|id| {
                id == &binding
                    || binding_to_module.get(id) == Some(&owner)
                    || external_imports.contains_key(id)
                    || metas[owner].local_import_bindings.contains(id)
                    || (entry_only
                        && decl_index_by_binding.contains_key(id)
                        && !binding_to_module.contains_key(id)
                        && !factory_preassigned_bindings.contains_key(id)
                        && !factory_importable_bindings.contains_key(id))
            })
        {
            continue;
        }
        entry_writers.entry(owner).or_default().insert(binding);
    }

    let mut owned_support_by_index: HashMap<usize, HashSet<BindingId>> = HashMap::default();
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
            .chain(entry_writers.get(&mi).into_iter().flatten())
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
                || (decl_index >= module_start
                    && !entry_writers
                        .get(&mi)
                        .is_some_and(|writers| writers.contains(&binding)))
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

            // An entry writer's read dependencies keep their existing owners.
            // Recursively adopting them could move a mutable entry function and
            // leave its reassignment targeting a new import in entry.js.
            let is_entry_writer = entry_writers
                .get(&mi)
                .is_some_and(|writers| writers.contains(&binding));
            for ref_binding in &item_infos[decl_index].references {
                if meta.referenced_bindings.insert(ref_binding.clone()) && !is_entry_writer {
                    queue.push(ref_binding.clone());
                }
            }

            let mut atom_collector = AtomRefCollector {
                candidate_atoms: reference_candidate_atoms,
                references: HashSet::default(),
                shadowed_atoms: vec![HashSet::default()],
            };
            analysis_items[decl_index].visit_with(&mut atom_collector);
            meta.referenced_atoms.extend(atom_collector.references);
        }
    }

    // Partition-time write analysis only sees statements after each namespace
    // boundary. esbuild commonly hoists writer functions before that boundary,
    // and the loop above adopts those functions as support declarations. Add
    // all their writes now: factory-state ownership and entry-import safety
    // both need the complete emitted body, including writes to entry-owned
    // bindings. Filter mixed declarations to this module's adopted
    // bindings before scanning, matching the source-item filtering below.
    for meta in metas.iter_mut() {
        if meta.owned_support_bindings.is_empty() {
            continue;
        }
        let owned_atoms: HashSet<Atom> = meta
            .owned_support_bindings
            .iter()
            .map(|(atom, _)| atom.clone())
            .collect();
        let mut indices: Vec<usize> = meta
            .owned_support_bindings
            .iter()
            .filter_map(|binding| decl_index_by_binding.get(binding).copied())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        for index in indices {
            let Some(item) = filter_item_to_owned_bindings(&analysis_items[index], &owned_atoms)
            else {
                continue;
            };
            meta.written_atoms.extend(
                exact_write_bindings_for_item(&item, &metadata.top_level_bindings)
                    .into_iter()
                    .map(|binding| binding.0),
            );
        }
    }

    let owned_support_source_items =
        retain_owned_support_source_items(source_slots, &owned_support_by_index);
    for index in owned_support_by_index.keys() {
        if source_slots[*index].is_none() {
            consumed.insert(*index);
        }
    }
    for index in drop_unowned_helper_sibling_indices {
        // Mixed helper declarations are filtered before scope partitioning.
        // Only discard a surviving sibling when it remains entry-owned; a
        // scope boundary may have claimed the filtered statement as module
        // body, namespace setup, or export setup.
        if consumed.contains(index) {
            continue;
        }
        if source_slots.get(*index).and_then(Option::as_ref).is_some() {
            source_slots[*index] = None;
            consumed.insert(*index);
        }
    }

    owned_support_source_items
}

/// Collect bindings read while a module body evaluates: everything outside
/// function, arrow, method, accessor, and class-instance-member bodies.
/// Computed member keys, static class members, `extends` clauses, and the
/// bodies of immediately invoked function/arrow expressions (direct call,
/// `new`, `.call`/`.apply`) evaluate with the enclosing statement and
/// therefore count as eager. A deferred body that some other module invokes
/// synchronously before the entry evaluates is not tracked.
struct EagerRefCollector {
    pub(super) references: HashSet<BindingId>,
}

impl EagerRefCollector {
    /// Visit the body of a function/arrow expression that runs immediately.
    fn visit_invoked_callee(&mut self, callee: &Expr) -> bool {
        match strip_parens(callee) {
            Expr::Arrow(arrow) => {
                arrow.params.visit_with(self);
                arrow.body.visit_with(self);
                true
            }
            Expr::Fn(fn_expr) => {
                fn_expr.function.params.visit_with(self);
                fn_expr.function.body.visit_with(self);
                true
            }
            Expr::Member(member) => {
                let is_call_or_apply = matches!(
                    &member.prop,
                    MemberProp::Ident(name) if matches!(name.sym.as_ref(), "call" | "apply")
                );
                is_call_or_apply && self.visit_invoked_callee(&member.obj)
            }
            _ => false,
        }
    }
}

impl Visit for EagerRefCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.references.insert((ident.sym.clone(), ident.ctxt));
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    // `export { f }` names a binding without evaluating it.
    fn visit_export_specifier(&mut self, _: &ExportSpecifier) {}

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if !self.visit_invoked_callee(callee) {
                callee.visit_with(self);
            }
        }
        call.args.visit_with(self);
    }

    fn visit_new_expr(&mut self, new_expr: &swc_core::ecma::ast::NewExpr) {
        if !self.visit_invoked_callee(&new_expr.callee) {
            new_expr.callee.visit_with(self);
        }
        new_expr.args.visit_with(self);
    }

    fn visit_getter_prop(&mut self, prop: &swc_core::ecma::ast::GetterProp) {
        prop.key.visit_with(self);
    }

    fn visit_setter_prop(&mut self, prop: &swc_core::ecma::ast::SetterProp) {
        prop.key.visit_with(self);
    }

    fn visit_class_member(&mut self, member: &swc_core::ecma::ast::ClassMember) {
        use swc_core::ecma::ast::ClassMember;
        match member {
            ClassMember::StaticBlock(_) => member.visit_children_with(self),
            ClassMember::ClassProp(prop) if prop.is_static => member.visit_children_with(self),
            ClassMember::PrivateProp(prop) if prop.is_static => member.visit_children_with(self),
            // Instance members defer their bodies and initializers, but a
            // computed key is evaluated while the class is defined.
            ClassMember::Method(method) => method.key.visit_with(self),
            ClassMember::ClassProp(prop) => prop.key.visit_with(self),
            ClassMember::AutoAccessor(accessor) => accessor.key.visit_with(self),
            _ => {}
        }
    }
}

/// How a scope module's body reaches each top-level binding it references.
///
/// `eager`: read while the module evaluates (see [`EagerRefCollector`]).
/// `guards`: every other reference sits inside a function or class that is
/// the whole initializer of a top-level binding (`function f`, `class C`,
/// `const f = () => ...`); the guard is that binding. Such a body can only
/// run through its guard. Deferral requires that guard to be unreachable from
/// eager or unknown-timing references, including transitive guard references.
/// `unprovable`: a deferred reference with no such guard (a function
/// expression flowing into an eager expression, an object-literal method or
/// getter at top level, a multi-declarator statement, ...). Its timing is not
/// known, so it is treated as unsafe.
struct ScopeEvaluationProfile {
    eager: HashSet<BindingId>,
    guards: HashMap<BindingId, HashSet<BindingId>>,
    unprovable: HashSet<BindingId>,
}

fn scope_evaluation_profile(
    meta: &ScopeModuleMeta,
    analysis_items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    decl_index_by_binding: &HashMap<BindingId, usize>,
) -> ScopeEvaluationProfile {
    let mut profile = ScopeEvaluationProfile {
        eager: HashSet::default(),
        guards: HashMap::default(),
        unprovable: HashSet::default(),
    };
    // Adopted hoisted functions have the same deferred-body guard as ordinary
    // body declarations. Other adopted support shapes remain unclassified.
    let adopted_functions = meta.owned_support_bindings.iter().filter_map(|binding| {
        let i = *decl_index_by_binding.get(binding)?;
        matches!(
            &analysis_items[i],
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_)))
        )
        .then_some(i)
    });
    for i in meta.body_indices.iter().copied().chain(adopted_functions) {
        let item = &analysis_items[i];
        let mut eager = EagerRefCollector {
            references: HashSet::default(),
        };
        item.visit_with(&mut eager);
        let guard = item_guard_binding(item);
        for reference in &item_infos[i].references {
            if eager.references.contains(reference) {
                profile.eager.insert(reference.clone());
            } else if let Some(guard) = &guard {
                profile
                    .guards
                    .entry(reference.clone())
                    .or_default()
                    .insert(guard.clone());
            } else {
                profile.unprovable.insert(reference.clone());
            }
        }
    }
    profile
}

/// The top-level binding whose function or class body owns every deferred
/// reference in `item`, when the item is exactly one such declaration.
fn item_guard_binding(item: &ModuleItem) -> Option<BindingId> {
    let decl = match item {
        ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => &export.decl,
        _ => return None,
    };
    match decl {
        Decl::Fn(function) => Some((function.ident.sym.clone(), function.ident.ctxt)),
        Decl::Class(class) => Some((class.ident.sym.clone(), class.ident.ctxt)),
        Decl::Var(var) => {
            let [declarator] = var.decls.as_slice() else {
                return None;
            };
            let Pat::Ident(binding) = &declarator.name else {
                return None;
            };
            let init = strip_parens(declarator.init.as_deref()?);
            matches!(init, Expr::Fn(_) | Expr::Arrow(_) | Expr::Class(_))
                .then(|| (binding.id.sym.clone(), binding.id.ctxt))
        }
        _ => None,
    }
}

/// Conservative reachability, not a call graph: touching a guard exposes all
/// references in its body, whether it is called, aliased, or passed elsewhere.
/// Unknown timing is also a root. Namespace exposure makes its exports reachable.
/// A worklist reaches a fixed point even for mutually recursive guards; a cycle
/// with no early/unknown root remains deferred.
fn early_scope_references(
    metas: &[ScopeModuleMeta],
    profiles: &[ScopeEvaluationProfile],
) -> HashSet<BindingId> {
    let mut dependents: HashMap<BindingId, HashSet<BindingId>> = HashMap::default();
    let mut pending = Vec::new();
    for (meta, profile) in metas.iter().zip(profiles) {
        pending.extend(profile.eager.iter().cloned());
        pending.extend(profile.unprovable.iter().cloned());
        // Adopted support declarations may not have a body-index profile.
        // Missing classification cannot certify one of their callees as deferred.
        pending.extend(
            meta.referenced_bindings
                .iter()
                .filter(|reference| {
                    !profile.eager.contains(*reference)
                        && !profile.guards.contains_key(*reference)
                        && !profile.unprovable.contains(*reference)
                })
                .cloned(),
        );
        for (reference, guards) in &profile.guards {
            for guard in guards {
                dependents
                    .entry(guard.clone())
                    .or_default()
                    .insert(reference.clone());
            }
        }
        for namespace in &meta.namespaces {
            dependents
                .entry(namespace.namespace_binding.clone())
                .or_default()
                .extend(meta.exported_bindings.iter().cloned());
        }
    }
    let mut reachable = HashSet::default();
    while let Some(binding) = pending.pop() {
        if reachable.insert(binding.clone()) {
            if let Some(references) = dependents.get(&binding) {
                pending.extend(references.iter().cloned());
            }
        }
    }
    reachable
}

/// Entry-owned hoisted function declarations that a scope module may call
/// while evaluating: the transitive top-level references of the function
/// stay within other such functions and the entry's own external imports.
/// Anything else (entry state, factory state, scope-module bindings) could be
/// read before it is initialized, so the function is not callable early.
fn self_contained_entry_functions(
    remaining_indices: &[usize],
    analysis_items: &[ModuleItem],
    item_infos: &[ItemBindingInfo],
    external_imports: &HashMap<BindingId, ExternalImport>,
) -> HashSet<BindingId> {
    let function_items: HashMap<&BindingId, usize> = remaining_indices
        .iter()
        .filter(|&&i| {
            matches!(
                &analysis_items[i],
                ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_)))
                    | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                        decl: Decl::Fn(_),
                        ..
                    }))
            )
        })
        .flat_map(|&i| {
            item_infos[i]
                .declared
                .iter()
                .map(move |binding| (binding, i))
        })
        .collect();

    // Start from every hoisted function and drop any whose references
    // escape the set until nothing changes.
    let mut safe: HashSet<BindingId> = function_items.keys().map(|b| (*b).clone()).collect();
    loop {
        let escaping: Vec<BindingId> = safe
            .iter()
            .filter(|binding| {
                let item = function_items[*binding];
                !item_infos[item].references.iter().all(|reference| {
                    reference == *binding
                        || safe.contains(reference)
                        || external_imports.contains_key(reference)
                })
            })
            .cloned()
            .collect();
        if escaping.is_empty() {
            break;
        }
        for binding in escaping {
            safe.remove(&binding);
        }
    }
    safe
}

fn compute_scope_imports_exports(
    analysis_items: &[ModuleItem],
    metadata: &ScopeMetadata,
    partition: &ScopePartition<'_>,
    maps: &ScopeBindingMaps,
    refs: &ScopeExtractionRefs<'_>,
) -> ScopeImportExportMaps {
    let span = tracing::info_span!("esbuild: scope compute imports exports");
    let _enter = span.enter();
    let ScopePartition {
        source_slots,
        metas,
        consumed,
        consumed_ns,
        ..
    } = partition;
    let item_infos = &metadata.item_infos;
    let boundaries = &metadata.boundaries;
    let binding_to_module = &maps.binding_to_module;
    let factory_preassigned_by_atom = &maps.factory_preassigned_by_atom;
    let factory_referenced = refs.factory_referenced;
    let factory_preassigned_bindings = refs.factory_preassigned_bindings;
    let factory_importable_bindings = refs.factory_importable_bindings;

    let binding_module_by_atom = atom_to_module_binding_map(binding_to_module);

    // Collect remaining entry references early so they feed into the
    // effective-export expansion below.
    let remaining_indices: Vec<usize> = (0..source_slots.len())
        .filter(|i| !consumed.contains(i))
        .collect();

    let mut entry_referenced: HashSet<BindingId> = HashSet::default();
    for &i in &remaining_indices {
        entry_referenced.extend(item_infos[i].references.iter().cloned());
    }
    for &(ns_idx, call_idx, _) in consumed_ns.iter() {
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

    let mut claimed_factory_filenames: HashMap<String, String> = HashMap::default();
    let mut conflicted_factory_filenames: HashSet<String> = HashSet::default();
    for meta in metas.iter() {
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
    let mut scope_claimed_factory_bindings: HashMap<BindingId, String> = HashMap::default();
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
    // Claiming an init factory removes its standalone output file, so every
    // importable support binding owned by that factory must follow the same
    // relocation as its written state. Otherwise sibling scope modules can
    // retain imports from a filename that no longer exists.
    let relocated_factory_importable_bindings: HashMap<BindingId, String> =
        factory_importable_bindings
            .iter()
            .map(|(binding, filename)| {
                let owner_filename = binding_to_module
                    .get(binding)
                    .and_then(|module_index| metas.get(*module_index))
                    .map(|meta| meta.filename.clone())
                    .unwrap_or_else(|| {
                        claimed_factory_filenames
                            .get(filename)
                            .unwrap_or(filename)
                            .clone()
                    });
                (binding.clone(), owner_filename)
            })
            .collect();
    let factory_binding_filename_by_atom =
        atom_to_filename_binding_map(&relocated_factory_importable_bindings);
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
    for boundary in boundaries.iter() {
        if factory_referenced.contains(&boundary.ns_binding) {
            binding_to_filename
                .entry(boundary.ns_binding.clone())
                .or_insert_with(|| "entry.js".to_string());
        }
    }

    // Scope modules can reference bindings that never leave the entry: a
    // declaration the last-module boundary search left in the remainder, or
    // the restored namespace object of a lazy module (the `import()`
    // lowering `then(() => (init_x(), ns_x))`). Neither is in
    // `binding_to_module`, so without this edge the consumer keeps a free
    // identifier. Record them so the consumer imports from the entry and the
    // entry exports them.
    //
    // The edge creates an entry <-> module import cycle in which the module
    // evaluates first, so it is only added where evaluation order provably
    // cannot observe the entry's initializers (see `ScopeEvaluationProfile`):
    // every scope-module use sits in a function or class bound to a top-level
    // name unreachable through early or unknown-timing guard references, or
    // the owner is a hoisted function whose transitive references stay within
    // such functions and external imports. Not being found by the eager collector is not a
    // proof of deferral; a reference whose timing cannot be proven, an eager
    // read of entry state, or a write keeps its previous unlinked shape and
    // the output validator reports it as unresolved.
    let entry_declared: HashSet<&BindingId> = remaining_indices
        .iter()
        .flat_map(|&i| item_infos[i].declared.iter())
        .collect();
    let early_callable_functions = self_contained_entry_functions(
        &remaining_indices,
        analysis_items,
        item_infos,
        &metadata.external_imports,
    );
    let consumed_namespace_bindings: HashSet<&BindingId> = consumed_ns
        .iter()
        .map(|(_, _, boundary)| &boundary.ns_binding)
        .collect();
    let profiles: Vec<ScopeEvaluationProfile> = metas
        .iter()
        .map(|meta| {
            scope_evaluation_profile(
                meta,
                analysis_items,
                item_infos,
                &maps.decl_index_by_binding,
            )
        })
        .collect();
    let early_references = early_scope_references(metas, &profiles);
    let mut scope_needed_entry_bindings: HashSet<BindingId> = HashSet::default();
    let mut scope_entry_imports = vec![HashSet::default(); metas.len()];
    for (mi, (meta, profile)) in metas.iter().zip(&profiles).enumerate() {
        let namespace_read_early = meta
            .namespaces
            .iter()
            .any(|namespace| early_references.contains(&namespace.namespace_binding));
        for binding in meta
            .referenced_bindings
            .iter()
            .chain(meta.exported_bindings.iter())
        {
            // Factory-owned state and support declarations are relocated
            // out of the entry later; they already have an owner filename.
            if meta.declared_bindings.contains(binding)
                || binding_to_module.contains_key(binding)
                || factory_preassigned_bindings.contains_key(binding)
                || factory_importable_bindings.contains_key(binding)
            {
                continue;
            }
            if !(entry_declared.contains(binding) || consumed_namespace_bindings.contains(binding))
            {
                continue;
            }
            let safe = if meta.written_atoms.contains(&binding.0) {
                false
            } else if profile.eager.contains(binding) {
                early_callable_functions.contains(binding)
            } else if profile.unprovable.contains(binding) || namespace_read_early {
                false
            } else {
                profile.guards.get(binding).is_some_and(|guards| {
                    guards.iter().all(|guard| !early_references.contains(guard))
                })
            };
            if safe {
                scope_needed_entry_bindings.insert(binding.clone());
                scope_entry_imports[mi].insert(binding.clone());
            }
        }
    }
    for binding in &scope_needed_entry_bindings {
        binding_to_filename
            .entry(binding.clone())
            .or_insert_with(|| "entry.js".to_string());
    }

    ScopeImportExportMaps {
        remaining_indices,
        entry_referenced,
        scope_needed_entry_bindings,
        scope_entry_imports,
        effective_exports,
        binding_to_filename,
        scope_claimed_factory_bindings,
        factory_binding_filename_by_atom,
        binding_filename_by_atom,
        filename_to_module,
        binding_module_by_atom,
    }
}

fn emit_scope_modules(
    cm: Lrc<SourceMap>,
    metadata: &ScopeMetadata,
    partition: &mut ScopePartition<'_>,
    maps: &ScopeBindingMaps,
    ie: &ScopeImportExportMaps,
    owned_support_source_items: &HashMap<usize, ModuleItem>,
    refs: &ScopeExtractionRefs<'_>,
) -> ScopeEmittedModules {
    // Step 4 (pass 2): emit each module with synthesized imports/exports.
    let span = tracing::info_span!("esbuild: scope emit modules", count = partition.metas.len());
    let _enter = span.enter();
    let ScopePartition {
        source_slots,
        metas,
        ..
    } = partition;
    let external_imports = &metadata.external_imports;
    let binding_to_module = &maps.binding_to_module;
    let decl_index_by_binding = &maps.decl_index_by_binding;
    let ScopeImportExportMaps {
        effective_exports,
        binding_to_filename,
        factory_binding_filename_by_atom,
        binding_filename_by_atom,
        filename_to_module,
        binding_module_by_atom,
        scope_entry_imports,
        ..
    } = ie;
    let factory_preassigned_bindings = refs.factory_preassigned_bindings;

    let mut module_local_atoms: HashMap<String, HashSet<Atom>> = HashMap::default();
    let mut modules = Vec::new();
    let mut module_referenced_atoms: HashMap<String, HashSet<Atom>> = HashMap::default();

    for (mi, meta) in metas.iter().enumerate() {
        let mut module_items: Vec<ModuleItem> = Vec::new();

        // Synthesize imports from other scope-hoisted modules.
        let declared_atoms: HashSet<Atom> = meta
            .declared_bindings
            .iter()
            .map(|(atom, _)| atom.clone())
            .collect();
        let mut imports_by_source: HashMap<usize, Vec<BindingId>> = HashMap::default();
        let mut imports_by_filename: HashMap<String, Vec<BindingId>> = HashMap::default();
        let mut external_import_bindings: HashSet<BindingId> = HashSet::default();
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
            } else if scope_entry_imports[mi].contains(ref_binding) {
                imports_by_filename
                    .entry("entry.js".to_string())
                    .or_default()
                    .push(ref_binding.clone());
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
            } else if scope_entry_imports[mi].contains(export_binding) {
                imports_by_filename
                    .entry("entry.js".to_string())
                    .or_default()
                    .push(export_binding.clone());
            } else if external_imports.contains_key(export_binding)
                && !meta.local_import_bindings.contains(export_binding)
            {
                external_import_bindings.insert(export_binding.clone());
            }
        }
        let mut external_import_bindings: Vec<BindingId> =
            external_import_bindings.into_iter().collect();
        external_import_bindings.sort_by(|a, b| a.0.cmp(&b.0));
        let mut external_imported_atoms = HashSet::default();
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
        let mut imported_atoms = HashSet::default();
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
            module_items.push(make_named_import_stmt_with_aliases(
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
            module_items.push(make_named_import_stmt_with_aliases(&names, &rel_path));
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
            decl_index_by_binding,
            owned_support_source_items,
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
            module_items.push(make_named_export_stmt(&names));
        }

        rename_bindings(&mut module_items, &import_renames);
        let mut code = emit_items(
            module_items,
            meta.filename.clone(),
            cm.clone(),
            refs.positions,
        );
        for namespace in &meta.namespaces {
            if !effective_exports[mi].contains(&namespace.namespace_binding.0) {
                continue;
            }
            let mut namespace_items =
                vec![make_namespace_object_decl(&namespace.namespace_binding.0)];
            namespace_items.extend(make_namespace_define_property_items(
                &namespace.namespace_binding.0,
                &namespace.export_entries,
            ));
            code.push_str("\n");
            code.push_mapped(emit_items(
                namespace_items,
                meta.filename.clone(),
                cm.clone(),
                refs.positions,
            ));
        }
        modules.push(UnpackedModule {
            id: meta.id.clone(),
            is_entry: false,
            code: code.code,
            filename: meta.filename.clone(),
            source_ranges: spans_byte_ranges(&cm, body_spans.into_iter()),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: code.points,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        });
    }

    ScopeEmittedModules {
        modules,
        module_local_atoms,
        module_referenced_atoms,
    }
}

fn build_scope_entry(
    metadata: &ScopeMetadata,
    partition: &mut ScopePartition<'_>,
    maps: &ScopeBindingMaps,
    ie: &ScopeImportExportMaps,
    refs: &ScopeExtractionRefs<'_>,
) -> (HashMap<String, HashSet<BindingId>>, Vec<ModuleItem>) {
    let span = tracing::info_span!("esbuild: scope build entry");
    let _enter = span.enter();
    let ScopePartition {
        source_slots,
        metas,
        consumed_ns,
        removable_export_helper_indices,
        ..
    } = partition;
    let external_imports = &metadata.external_imports;
    let binding_to_module = &maps.binding_to_module;
    let ScopeImportExportMaps {
        remaining_indices,
        entry_referenced,
        scope_needed_entry_bindings,
        binding_module_by_atom,
        ..
    } = ie;
    let factory_referenced = refs.factory_referenced;

    // Track which external bindings each scope-hoisted module already imports
    // (used later to avoid duplicate imports when merging init factories).
    let mut module_already_imports: HashMap<String, HashSet<BindingId>> = HashMap::default();
    for meta in metas.iter() {
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
    // `export { ... }` or `export <decl>` declarations.  Used below to avoid
    // synthesizing duplicate exports for namespace and entry-owned bindings.
    // Only count unaliased exports: `export { ns_a }` makes `ns_a`
    // importable by name, but `export { ns_a as math }` does not —
    // consumers would need `import { math }`, not `import { ns_a }`.
    let entry_already_exports: HashSet<Atom> = remaining_indices
        .iter()
        .flat_map(|&i| match source_slots[i].as_ref().unwrap() {
            item @ ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(_)) => {
                module_item_declared_binding_ids(item)
                    .into_iter()
                    .map(|(atom, _)| atom)
                    .collect::<Vec<_>>()
            }
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
    let mut synthesized_entry_exports: Vec<Atom> = Vec::new();
    let mut restored_namespace_bindings: HashSet<BindingId> = HashSet::default();
    for &(ns_idx, call_idx, boundary) in consumed_ns.iter() {
        let entry_needs = entry_referenced.contains(&boundary.ns_binding);
        let factory_needs = factory_referenced.contains(&boundary.ns_binding);
        let scope_module_needs = scope_needed_entry_bindings.contains(&boundary.ns_binding);
        if !entry_needs && !factory_needs && !scope_module_needs {
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
        // (`make_namespace_object_decl` + these defineProperty items).
        restored_items.extend(make_namespace_define_property_items(
            &boundary.ns_binding.0,
            &boundary.export_entries,
        ));
        // If a factory or scope module references this namespace but the
        // entry doesn't already export it via an ESM export declaration,
        // synthesize one so `import { ns_a } from "./entry.js"` resolves.
        if (factory_needs || scope_module_needs)
            && !entry_already_exports.contains(&boundary.ns_binding.0)
        {
            synthesized_entry_exports.push(boundary.ns_binding.0.clone());
        }
    }
    // Remaining entry declarations that scope modules import from the entry.
    for (atom, _) in scope_needed_entry_bindings
        .iter()
        .filter(|binding| !restored_namespace_bindings.contains(*binding))
    {
        if !entry_already_exports.contains(atom) {
            synthesized_entry_exports.push(atom.clone());
        }
    }
    for index in removable_export_helper_indices.iter() {
        let _ = source_slots[*index].take();
    }
    if !synthesized_entry_exports.is_empty() {
        synthesized_entry_exports.sort();
        synthesized_entry_exports.dedup();
        restored_items.push(make_named_export_stmt(&synthesized_entry_exports));
    }
    let mut entry_imports: HashMap<usize, Vec<BindingId>> = HashMap::default();
    for ref_binding in entry_referenced.iter() {
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
            remaining.push(make_named_import_stmt_with_aliases(
                &names,
                &metas[source_mi].filename,
            ));
        }
    }
    // Factory ownership is provisional until phase 6 finishes merging and
    // demotion. Its final repair_entry_imports pass adds the entry edges using
    // surviving owners, after relocated declarations have been removed. Doing
    // this here leaves stale imports (and needless aliases) when a factory
    // returns to entry. Scope-module imports above already have final owners.
    rename_bindings(&mut entry_tail, &entry_import_renames);
    remaining.extend(entry_tail);

    (module_already_imports, remaining)
}
