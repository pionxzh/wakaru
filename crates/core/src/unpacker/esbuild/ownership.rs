//! Factory ownership planning: which output file declares every factory,
//! support declaration, and piece of mutable state, decided before emission.

use swc_core::atoms::Atom;
use swc_core::common::Span;
use swc_core::ecma::ast::{Decl, ModuleItem, Stmt};

use crate::collections::{HashMap, HashSet};
use crate::unpacker::{BindingId, UnpackedModule};

use super::bindings::{atom_binding_map_from_keys, atom_to_filename_binding_map, ExternalImport};
use super::factories::CjsFactoryParams;
use super::synthesis::{factory_owned_export_names, relative_import_path};

/// A detected factory with its output filename and the top-level bindings its
/// resolved body reads and writes, waiting for an ownership decision.
pub(super) struct PendingFactory {
    pub(super) binding: BindingId,
    pub(super) var_name: Atom,
    pub(super) filename: String,
    pub(super) cjs_params: Option<CjsFactoryParams>,
    pub(super) body_stmts: Vec<Stmt>,
    pub(super) referenced_bindings: HashSet<BindingId>,
    pub(super) write_bindings: HashSet<BindingId>,
    pub(super) span: Span,
}

/// Which top-level bindings a factory may claim as support declarations.
/// Runtime helpers and factory bindings stay with the bundle runtime, and a
/// binding without a top-level declaration has nothing to move.
pub(super) struct SupportClaimFilter<'a> {
    pub(super) helper_syms: &'a HashSet<Atom>,
    pub(super) factory_syms: &'a HashSet<Atom>,
    pub(super) top_level_decl_indices: &'a HashMap<BindingId, usize>,
}

impl SupportClaimFilter<'_> {
    pub(super) fn admits(&self, binding: &BindingId) -> bool {
        !self.helper_syms.contains(&binding.0)
            && !self.factory_syms.contains(&binding.0)
            && self.top_level_decl_indices.contains_key(binding)
    }
}

/// Assigns each standalone factory its own binding, the state its body writes,
/// and the support declarations it reaches. Bindings that an earlier owner
/// (a scope module or a merged init) already holds are left alone.
///
/// Claims are first-come in factory order: every factory first claims the
/// declarations its body references directly, then the reference closure
/// expands one level per round across all factories until nothing changes.
pub(super) fn claim_standalone_ownership(
    standalone_factories: &[PendingFactory],
    top_level_decl_references: &HashMap<BindingId, HashSet<BindingId>>,
    filter: &SupportClaimFilter<'_>,
    binding_to_filename: &mut HashMap<BindingId, String>,
    factory_owned_bindings: &mut HashMap<String, HashSet<BindingId>>,
) {
    for factory in standalone_factories {
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
    for factory in standalone_factories {
        for ref_binding in &factory.referenced_bindings {
            if factory.write_bindings.contains(ref_binding)
                || binding_to_filename.contains_key(ref_binding)
                || !filter.admits(ref_binding)
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
        for factory in standalone_factories {
            let owned_bindings = factory_owned_bindings
                .get(&factory.filename)
                .cloned()
                .unwrap_or_default();
            for owned_binding in owned_bindings {
                for ref_binding in top_level_decl_references
                    .get(&owned_binding)
                    .into_iter()
                    .flatten()
                {
                    if binding_to_filename.contains_key(ref_binding) || !filter.admits(ref_binding)
                    {
                        continue;
                    }
                    binding_to_filename.insert(ref_binding.clone(), factory.filename.clone());
                    factory_owned_bindings
                        .entry(factory.filename.clone())
                        .or_default()
                        .insert(ref_binding.clone());
                    changed = true;
                }
            }
        }
    }
}

/// Standalone factories joined by state-write edges into output groups.
pub(super) struct WriterGroups {
    /// Original filename → canonical group filename, for every member that is
    /// not its group's canonical file.
    pub(super) redirects: HashMap<String, String>,
    /// Canonical filenames of groups that contain a writer of owned state.
    pub(super) affected: HashSet<String>,
}

/// Joins standalone factories that write state owned by another standalone
/// factory, either directly from the factory body or through a support
/// declaration the factory owns. Each group's canonical file is its member
/// that comes first in factory order.
pub(super) fn union_writer_groups(
    standalone_factories: &[PendingFactory],
    binding_to_filename: &HashMap<BindingId, String>,
    factory_owned_bindings: &HashMap<String, HashSet<BindingId>>,
    top_level_decl_writes: &HashMap<BindingId, HashSet<BindingId>>,
) -> WriterGroups {
    fn find(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }

    let factory_index_by_filename: HashMap<&str, usize> = standalone_factories
        .iter()
        .enumerate()
        .map(|(index, factory)| (factory.filename.as_str(), index))
        .collect();
    let owner_index = |binding: &BindingId| {
        binding_to_filename
            .get(binding)
            .and_then(|filename| factory_index_by_filename.get(filename.as_str()))
            .copied()
    };

    // Union-find whose root is always the smallest member index.
    let mut parent: Vec<usize> = (0..standalone_factories.len()).collect();
    let mut writer_indices = Vec::new();
    for (writer_index, factory) in standalone_factories.iter().enumerate() {
        // A factory body that assigns top-level state directly is a writer of
        // that state too. Join it to the state's owner so the group declares
        // the binding once instead of every writing factory keeping a copy.
        let direct_writes = factory.write_bindings.iter();
        let support_writes = factory_owned_bindings
            .get(&factory.filename)
            .into_iter()
            .flatten()
            .flat_map(|owned| top_level_decl_writes.get(owned).into_iter().flatten());
        let mut is_writer = false;
        for owner in direct_writes.chain(support_writes).filter_map(owner_index) {
            is_writer = true;
            let left = find(&mut parent, writer_index);
            let right = find(&mut parent, owner);
            parent[left.max(right)] = left.min(right);
        }
        if is_writer {
            writer_indices.push(writer_index);
        }
    }

    let mut groups = WriterGroups {
        redirects: HashMap::default(),
        affected: HashSet::default(),
    };
    for index in 0..standalone_factories.len() {
        let root = find(&mut parent, index);
        if root != index {
            groups.redirects.insert(
                standalone_factories[index].filename.clone(),
                standalone_factories[root].filename.clone(),
            );
        }
    }
    for index in writer_indices {
        let root = find(&mut parent, index);
        groups
            .affected
            .insert(standalone_factories[root].filename.clone());
    }
    groups
}

pub(super) fn canonical_factory_filename<'a>(
    redirects: &'a HashMap<String, String>,
    filename: &'a str,
) -> &'a str {
    redirects.get(filename).map_or(filename, String::as_str)
}

/// Top-level facts about the bundle that ownership decisions read.
pub(super) struct TopLevelIndex {
    /// Item index of the first declaration of each top-level binding.
    pub(super) decl_indices: HashMap<BindingId, usize>,
    pub(super) decl_binding_by_atom: HashMap<Atom, BindingId>,
    /// Top-level bindings each declaration references.
    pub(super) decl_references: HashMap<BindingId, HashSet<BindingId>>,
    /// Top-level bindings each declaration writes.
    pub(super) decl_writes: HashMap<BindingId, HashSet<BindingId>>,
    pub(super) external_imports: HashMap<BindingId, ExternalImport>,
}

/// A top-level statement that writes a top-level binding, at any depth.
pub(super) struct TopLevelWriterItem {
    pub(super) source_index: usize,
    /// Top-level bindings this statement assigns, at any depth.
    pub(super) write_targets: HashSet<BindingId>,
    pub(super) referenced_bindings: HashSet<BindingId>,
    /// Bindings this item itself declares. A declaration owned by the
    /// target group moves with the ownership unit instead of relocating.
    pub(super) declared_bindings: HashSet<BindingId>,
    /// Plain statements can move between modules as a unit; declarations
    /// cannot (entry call sites would need an import back).
    pub(super) relocatable_shape: bool,
    pub(super) span: Span,
}

/// Where factory-related bindings and statements go. The planning steps
/// build it up in order; emission only reads it.
pub(super) struct FactoryOwnership {
    /// Output file of every binding that some recovered module declares.
    pub(super) binding_to_filename: HashMap<BindingId, String>,
    /// Support declarations and state each output file declares and exports.
    pub(super) factory_owned_bindings: HashMap<String, HashSet<BindingId>>,
    /// Non-canonical writer-group member file → canonical group file.
    pub(super) redirects: HashMap<String, String>,
    /// Canonical group files that hold a writer of their own state; entry
    /// copies of that state are dropped and re-imported.
    pub(super) affected: HashSet<String>,
    /// Entry statements that move into a standalone group.
    pub(super) relocated_writers: HashMap<String, Vec<TopLevelWriterItem>>,
    /// Declarations a merged scope module adopted; the entry drops its copy.
    pub(super) entry_duplicate_declarations: HashSet<BindingId>,
}

impl FactoryOwnership {
    pub(super) fn new(binding_to_filename: HashMap<BindingId, String>) -> Self {
        Self {
            binding_to_filename,
            factory_owned_bindings: HashMap::default(),
            redirects: HashMap::default(),
            affected: HashSet::default(),
            relocated_writers: HashMap::default(),
            entry_duplicate_declarations: HashSet::default(),
        }
    }

    /// Every file of an affected group: the canonical file and each
    /// redirected member.
    pub(super) fn affected_group_files(&self) -> impl Iterator<Item = &String> {
        self.affected.iter().chain(
            self.redirects
                .iter()
                .filter(|(_, canonical)| self.affected.contains(*canonical))
                .map(|(member, _)| member),
        )
    }

    /// Records `binding` as declared and exported by `filename`.
    pub(super) fn own(&mut self, binding: BindingId, filename: &str) {
        self.binding_to_filename
            .insert(binding.clone(), filename.to_string());
        self.factory_owned_bindings
            .entry(filename.to_string())
            .or_default()
            .insert(binding);
    }
}

/// Splits factories into init factories that merge into a scope module and
/// standalone factories.
///
/// If a factory writes to bindings that all belong to a single scope-hoisted
/// module, and that module claimed the written state, it is an init function
/// for that module. Merge its body into the target module rather than
/// emitting a separate file with invalid ESM (imports are read-only, so
/// `import {x} ...; x = ...` would be a runtime error).
pub(super) fn partition_merged_factories(
    pending_factories: Vec<PendingFactory>,
    scope_claimed_factory_bindings: &HashMap<BindingId, String>,
    factory_importable_bindings: &HashMap<BindingId, String>,
    index: &TopLevelIndex,
    ownership: &mut FactoryOwnership,
) -> (HashMap<String, Vec<MergedFactory>>, Vec<PendingFactory>) {
    let mut merged_factories: HashMap<String, Vec<MergedFactory>> = HashMap::default();
    let mut standalone_factories: Vec<PendingFactory> = Vec::new();

    for factory in pending_factories {
        if factory.write_bindings.is_empty() {
            standalone_factories.push(factory);
            continue;
        }

        // Check if all write targets belong to the same scope-hoisted module.
        let mut target_filename: Option<String> = None;
        let mut is_single_target = true;
        for wb in &factory.write_bindings {
            if let Some(fname) = ownership.binding_to_filename.get(wb) {
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

        if let (true, Some(fname), true) =
            (is_single_target, target_filename, is_scope_claimed_init)
        {
            ownership
                .binding_to_filename
                .insert(factory.binding.clone(), fname.clone());
            // The standalone factory file also owns top-level support
            // declarations referenced by its body. When the factory is
            // absorbed, move and export those declarations with it so other
            // recovered modules can follow the relocated import edge.
            for (owned_binding, owner_filename) in factory_importable_bindings {
                if owner_filename != &factory.filename
                    || *owned_binding == factory.binding
                    || !index.decl_indices.contains_key(owned_binding)
                {
                    continue;
                }
                if ownership.binding_to_filename.contains_key(owned_binding) {
                    continue;
                }
                ownership.own(owned_binding.clone(), &fname);
            }
            for write_binding in &factory.write_bindings {
                let owned_binding = index
                    .decl_binding_by_atom
                    .get(&write_binding.0)
                    .unwrap_or(write_binding);
                if scope_claimed_factory_bindings.contains_key(write_binding)
                    || scope_claimed_factory_bindings.contains_key(owned_binding)
                {
                    ownership.own(owned_binding.clone(), &fname);
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
    (merged_factories, standalone_factories)
}

/// Joins standalone factories into writer groups and renames every member,
/// owner entry, and owned-binding table to its group's canonical file.
///
/// A factory can adopt a hoisted support declaration whose body writes state
/// initialized by another lazy factory. Those factories cannot be separate
/// ESM modules: the adopted function would assign to a read-only import.
/// CommonJS factories participate too: emission retains each factory's
/// callable/cache boundary.
pub(super) fn group_standalone_writers(
    standalone_factories: &mut [PendingFactory],
    index: &TopLevelIndex,
    ownership: &mut FactoryOwnership,
) {
    let WriterGroups {
        redirects,
        affected,
    } = union_writer_groups(
        standalone_factories,
        &ownership.binding_to_filename,
        &ownership.factory_owned_bindings,
        &index.decl_writes,
    );
    for factory in standalone_factories.iter_mut() {
        if let Some(canonical) = redirects.get(&factory.filename) {
            factory.filename = canonical.clone();
        }
    }
    for filename in ownership.binding_to_filename.values_mut() {
        if let Some(canonical) = redirects.get(filename) {
            *filename = canonical.clone();
        }
    }
    if !redirects.is_empty() {
        let original_owned = std::mem::take(&mut ownership.factory_owned_bindings);
        for (filename, bindings) in original_owned {
            let canonical = canonical_factory_filename(&redirects, &filename).to_string();
            ownership
                .factory_owned_bindings
                .entry(canonical)
                .or_default()
                .extend(bindings);
        }
    }
    ownership.redirects = redirects;
    ownership.affected = affected;
}

fn standalone_group_filenames(standalone_factories: &[PendingFactory]) -> HashSet<String> {
    standalone_factories
        .iter()
        .map(|factory| factory.filename.clone())
        .collect()
}

/// Keeps every entry writer of standalone-group state with that state.
///
/// A support declaration can make a standalone factory the sole owner of
/// mutable state while top-level writers of that state remain in entry.js.
/// Import repair would then turn each writer into an assignment to an
/// immutable ESM import. The ownership unit must stay atomic: relocate a
/// writer statement to the owner when that is provably safe, and cancel the
/// group's standalone split (demotion) when it is not. A writer must never
/// stay behind against an imported binding.
///
/// Returns the groups that need demotion.
pub(super) fn place_top_level_writers(
    top_level_writer_items: Vec<TopLevelWriterItem>,
    source_items: &[ModuleItem],
    remaining_entry_spans: &HashSet<(u32, u32)>,
    standalone_factories: &[PendingFactory],
    index: &TopLevelIndex,
    ownership: &mut FactoryOwnership,
) -> HashSet<String> {
    let standalone_group_filenames = standalone_group_filenames(standalone_factories);
    let mut relocation_demoted_groups: HashSet<String> = HashSet::default();
    let reassigned_top_level_bindings: HashSet<BindingId> = top_level_writer_items
        .iter()
        .flat_map(|writer| writer.write_targets.iter().cloned())
        .collect();
    for writer in top_level_writer_items {
        if !remaining_entry_spans.contains(&(writer.span.lo.0, writer.span.hi.0)) {
            continue;
        }
        let target_owner_filenames: HashSet<&String> = writer
            .write_targets
            .iter()
            .filter_map(|target| ownership.binding_to_filename.get(target))
            .filter(|filename| standalone_group_filenames.contains(*filename))
            .collect();
        if target_owner_filenames.is_empty() {
            continue;
        }
        if target_owner_filenames.len() == 1 {
            let owner_filename = (*target_owner_filenames.iter().next().unwrap()).clone();
            // A declaration whose own bindings the group already owns moves
            // with the ownership unit (factory_owned_decl_items); it is not an
            // entry-resident writer.
            if !writer.declared_bindings.is_empty()
                && writer.declared_bindings.iter().all(|binding| {
                    ownership
                        .binding_to_filename
                        .get(binding)
                        .is_some_and(|filename| *filename == owner_filename)
                })
            {
                continue;
            }
            // A hoisted function declaration that writes the group's state
            // joins the ownership unit outright: the owner declares and
            // exports it, and entry call sites follow the import. Only a
            // stable binding qualifies; moving a reassigned function would
            // shift the import write from the state to the callable.
            if let (ModuleItem::Stmt(Stmt::Decl(Decl::Fn(_))), Some(binding)) = (
                &source_items[writer.source_index],
                writer.declared_bindings.iter().next().cloned(),
            ) {
                let targets_owned = writer.write_targets.iter().all(|target| {
                    ownership
                        .binding_to_filename
                        .get(target)
                        .is_some_and(|filename| *filename == owner_filename)
                });
                let deps_resolvable = !writer.referenced_bindings.iter().any(|ref_binding| {
                    ref_binding != &binding
                        && index.decl_indices.contains_key(ref_binding)
                        && !ownership.binding_to_filename.contains_key(ref_binding)
                        && !index.external_imports.contains_key(ref_binding)
                });
                if targets_owned
                    && deps_resolvable
                    && !reassigned_top_level_bindings.contains(&binding)
                    && !ownership.binding_to_filename.contains_key(&binding)
                {
                    ownership.own(binding, &owner_filename);
                    // The group now holds a writer of its own state, so entry
                    // copies of that unit must be dropped and re-imported.
                    ownership.affected.insert(owner_filename);
                    continue;
                }
            }
            // Relocatable iff the statement can move as a unit, every written
            // top-level binding belongs to this owner (a write to an
            // entry-owned binding could not follow), and every referenced
            // top-level binding has a concrete emitted owner to import from.
            let all_targets_owned = writer.write_targets.iter().all(|target| {
                ownership
                    .binding_to_filename
                    .get(target)
                    .is_some_and(|filename| *filename == owner_filename)
                    && ownership
                        .factory_owned_bindings
                        .get(&owner_filename)
                        .is_some_and(|owned| owned.contains(target))
            });
            let deps_resolvable = !writer.referenced_bindings.iter().any(|binding| {
                index.decl_indices.contains_key(binding)
                    && !ownership.binding_to_filename.contains_key(binding)
                    && !index.external_imports.contains_key(binding)
            });
            if writer.relocatable_shape && all_targets_owned && deps_resolvable {
                ownership
                    .relocated_writers
                    .entry(owner_filename)
                    .or_default()
                    .push(writer);
                continue;
            }
        }
        // Unrelocatable writer: leaving it behind produces an assignment to an
        // immutable import, so every involved group's split must be cancelled.
        relocation_demoted_groups.extend(target_owner_filenames.into_iter().cloned());
    }
    relocation_demoted_groups
}

/// Extends the requested demotions over every standalone group that depends
/// on a demoted binding, and returns the full set if it can be applied.
///
/// Demotion cancels a group's standalone split and re-synthesizes its init
/// functions into the entry, where the writers and owned declarations already
/// live. It cascades over standalone groups whose complete emission surface
/// references a demoted binding (their synthesized imports would dangle). If
/// a scope module or merged factory depends on a demoted binding, demotion
/// cannot be applied safely; that residual keeps today's shape and is
/// reported by output validation.
pub(super) fn plan_demotion(
    requested: HashSet<String>,
    standalone_factories: &[PendingFactory],
    merged_factories: &HashMap<String, Vec<MergedFactory>>,
    module_referenced_atoms: &HashMap<String, HashSet<Atom>>,
    remaining_entry_spans: &HashSet<(u32, u32)>,
    index: &TopLevelIndex,
    ownership: &FactoryOwnership,
) -> Option<HashSet<String>> {
    // Build the same dependency surface that standalone emission will use.
    // Factory bodies are only one source: adopted support declarations and
    // top-level writer statements scheduled for relocation can also require
    // a binding owned by another standalone group. If that provider is
    // demoted into entry, every such consumer must demote with it because
    // this pass cannot synthesize an import from entry.js. Build this map
    // only on the uncommon demotion path to avoid retaining another binding
    // graph for ordinary large bundles.
    let standalone_group_filenames = standalone_group_filenames(standalone_factories);
    let mut standalone_group_references: HashMap<String, HashSet<BindingId>> = HashMap::default();
    for factory in standalone_factories {
        standalone_group_references
            .entry(factory.filename.clone())
            .or_default()
            .extend(factory.referenced_bindings.iter().cloned());
    }
    for (filename, writers) in &ownership.relocated_writers {
        standalone_group_references
            .entry(filename.clone())
            .or_default()
            .extend(
                writers
                    .iter()
                    .flat_map(|writer| writer.referenced_bindings.iter().cloned()),
            );
    }
    for (filename, owned_bindings) in &ownership.factory_owned_bindings {
        if !standalone_group_filenames.contains(filename) {
            continue;
        }
        let references = standalone_group_references
            .entry(filename.clone())
            .or_default();
        for binding in owned_bindings {
            references.extend(
                index
                    .decl_references
                    .get(binding)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
    }

    let mut demoted = requested;
    loop {
        let demoted_bindings: HashSet<&BindingId> = ownership
            .binding_to_filename
            .iter()
            .filter(|(_, filename)| demoted.contains(*filename))
            .map(|(binding, _)| binding)
            .collect();
        let additions: Vec<String> = standalone_group_references
            .iter()
            .filter(|(filename, _)| !demoted.contains(*filename))
            .filter(|(_, references)| {
                references
                    .iter()
                    .any(|binding| demoted_bindings.contains(binding))
            })
            .map(|(filename, _)| filename.clone())
            .collect();
        if additions.is_empty() {
            break;
        }
        demoted.extend(additions);
    }

    let demoted_bindings: HashSet<BindingId> = ownership
        .binding_to_filename
        .iter()
        .filter(|(_, filename)| demoted.contains(*filename))
        .map(|(binding, _)| binding.clone())
        .collect();
    let demoted_atoms: HashSet<Atom> = demoted_bindings
        .iter()
        .map(|(atom, _)| atom.clone())
        .collect();
    let demotion_safe = standalone_factories.iter().all(|factory| {
        !demoted.contains(&factory.filename)
            // A partially filtered mixed declaration already left a
            // sibling in entry at the factory's own span.
            || !remaining_entry_spans.contains(&(factory.span.lo.0, factory.span.hi.0))
    }) && !merged_factories.values().flatten().any(|merged| {
        merged
            .referenced_bindings
            .iter()
            .any(|binding| demoted_bindings.contains(binding))
    }) && !module_referenced_atoms
        .values()
        .any(|atoms| atoms.iter().any(|atom| demoted_atoms.contains(atom)));
    demotion_safe.then_some(demoted)
}

/// Cancels the standalone split of every demoted group: its factories leave
/// the standalone list and its ownership records are dropped. Returns the
/// demoted factories, in factory order, for entry emission to restore.
pub(super) fn apply_demotion(
    demoted: &HashSet<String>,
    standalone_factories: &mut Vec<PendingFactory>,
    ownership: &mut FactoryOwnership,
) -> Vec<PendingFactory> {
    let (demoted_factories, kept_factories): (Vec<_>, Vec<_>) =
        std::mem::take(standalone_factories)
            .into_iter()
            .partition(|factory| demoted.contains(&factory.filename));
    *standalone_factories = kept_factories;
    ownership
        .binding_to_filename
        .retain(|_, filename| !demoted.contains(filename));
    for filename in demoted {
        ownership.factory_owned_bindings.remove(filename);
        ownership.affected.remove(filename);
    }
    // Compatibility aliases share the lifetime of their canonical
    // owner. A demoted owner is restored in entry, not emitted as a file.
    ownership
        .redirects
        .retain(|_, canonical| !demoted.contains(canonical));
    ownership
        .relocated_writers
        .retain(|filename, _| !demoted.contains(filename));
    demoted_factories
}

/// Plans what every merged factory brings into its scope module.
///
/// Planning walks modules in output order and each module's adopted
/// declarations become visible to the modules after it. Relocated support
/// declarations are cloned into their owner from the shared source items; the
/// same declaration may still occupy an entry slot. Every relocated binding
/// is recorded so entry emission drops the duplicate and re-imports the
/// owner's single mutable copy instead of silently forking the state.
pub(super) fn plan_merged_modules(
    modules: &[UnpackedModule],
    mut merged_factories: HashMap<String, Vec<MergedFactory>>,
    index: &TopLevelIndex,
    module_already_imports: &HashMap<String, HashSet<BindingId>>,
    module_local_atoms: &HashMap<String, HashSet<Atom>>,
    module_referenced_atoms: &HashMap<String, HashSet<Atom>>,
    ownership: &mut FactoryOwnership,
) -> Vec<MergedModulePlan> {
    if merged_factories.is_empty() {
        return Vec::new();
    }
    let binding_filename_by_atom = atom_to_filename_binding_map(&ownership.binding_to_filename);
    let external_import_by_atom = atom_binding_map_from_keys(&index.external_imports);
    let context = MergedPlanContext {
        index,
        external_import_by_atom: &external_import_by_atom,
        module_already_imports,
        module_local_atoms,
        module_referenced_atoms,
        pre_merge_binding_filename_by_atom: &binding_filename_by_atom,
    };
    let mut plans = Vec::new();
    for (module_index, module) in modules.iter().enumerate() {
        let Some(factories) = merged_factories.remove(&module.filename) else {
            continue;
        };
        plans.push(plan_merged_module(
            module_index,
            &module.filename,
            factories,
            &context,
            ownership,
        ));
    }
    plans
}

/// Entry bindings whose declarations moved to a group and must be dropped
/// from entry.js so the entry imports the group's single mutable copy.
pub(super) fn relocated_entry_bindings(
    standalone_factory_write_bindings: &HashSet<BindingId>,
    index: &TopLevelIndex,
    ownership: &FactoryOwnership,
) -> HashSet<BindingId> {
    let affected_owned_bindings: HashSet<BindingId> = ownership
        .affected
        .iter()
        .flat_map(|filename| {
            ownership
                .factory_owned_bindings
                .get(filename)
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    // Seed with every writer of relocated factory state — and with the
    // written state bindings themselves. A passive state declaration
    // (`var state;`) neither writes nor references anything, so the
    // reference closure below never reaches it; leaving its entry copy
    // behind while the owner declares and exports the same binding forks
    // the mutable state silently.
    let mut relocated_entry_bindings = ownership.entry_duplicate_declarations.clone();
    for binding in &affected_owned_bindings {
        // State assigned directly by a factory body has no support-writer
        // edge; the factory group owns its declaration all the same.
        if standalone_factory_write_bindings.contains(binding) {
            relocated_entry_bindings.insert(binding.clone());
        }
        let written_state: Vec<BindingId> = index
            .decl_writes
            .get(binding)
            .into_iter()
            .flatten()
            .filter(|written| {
                ownership
                    .binding_to_filename
                    .get(*written)
                    .is_some_and(|filename| ownership.affected.contains(filename))
            })
            .cloned()
            .collect();
        if written_state.is_empty() {
            continue;
        }
        relocated_entry_bindings.insert(binding.clone());
        relocated_entry_bindings.extend(written_state);
    }
    let mut changed = true;
    while changed {
        changed = false;
        for binding in &affected_owned_bindings {
            if relocated_entry_bindings.contains(binding)
                || !index
                    .decl_references
                    .get(binding)
                    .into_iter()
                    .flatten()
                    .any(|referenced| relocated_entry_bindings.contains(referenced))
            {
                continue;
            }
            relocated_entry_bindings.insert(binding.clone());
            changed = true;
        }
    }
    relocated_entry_bindings
}

/// A lazy init factory whose writes all belong to one scope module; its body
/// is appended to that module instead of becoming a file of its own.
pub(super) struct MergedFactory {
    pub(super) var_name: Atom,
    pub(super) cjs_params: Option<CjsFactoryParams>,
    pub(super) stmts: Vec<Stmt>,
    pub(super) referenced_bindings: HashSet<BindingId>,
    pub(super) write_bindings: HashSet<BindingId>,
}

/// Read-only inputs for planning merged factories.
pub(super) struct MergedPlanContext<'a> {
    pub(super) index: &'a TopLevelIndex,
    pub(super) external_import_by_atom: &'a HashMap<Atom, BindingId>,
    pub(super) module_already_imports: &'a HashMap<String, HashSet<BindingId>>,
    pub(super) module_local_atoms: &'a HashMap<String, HashSet<Atom>>,
    pub(super) module_referenced_atoms: &'a HashMap<String, HashSet<Atom>>,
    /// Binding owners by atom as they stood before any merged module adopted
    /// declarations. Existing-source import augmentation reads this snapshot.
    pub(super) pre_merge_binding_filename_by_atom: &'a HashMap<Atom, (BindingId, String)>,
}

/// What one scope module receives from the factories merged into it.
pub(super) struct MergedModulePlan {
    pub(super) module_index: usize,
    /// External imports to re-materialize, sorted by name.
    pub(super) external_imports: Vec<BindingId>,
    /// Relative specifier and imported names, sorted by source filename.
    pub(super) named_imports: Vec<(String, Vec<Atom>)>,
    /// Adopted support declarations: source item index and the binding names
    /// the module keeps from it, in source order.
    pub(super) owned_items: Vec<(usize, HashSet<Atom>)>,
    /// Sorted names the module exports for the declarations it owns. Later
    /// modules only add to their own owned sets, so this stays final.
    pub(super) export_names: Vec<Atom>,
    pub(super) init_bodies: Vec<(Atom, Option<CjsFactoryParams>, Vec<Stmt>)>,
    /// Names the synthesized init helpers must not shadow.
    pub(super) helper_reserved_atoms: HashSet<Atom>,
}

/// Plans the merged factories of one scope module and records the support
/// declarations it adopts in the ownership tables, so later modules resolve
/// those bindings to this module. Adopted declarations also go into
/// `entry_duplicate_declarations` so the entry drops its copy.
pub(super) fn plan_merged_module(
    module_index: usize,
    filename: &str,
    factories: Vec<MergedFactory>,
    context: &MergedPlanContext<'_>,
    ownership: &mut FactoryOwnership,
) -> MergedModulePlan {
    let index = context.index;
    let (mut extra_imports, extra_external_imports, extra_owned_bindings, init_bodies) = {
        let current_binding_filename_by_atom =
            atom_to_filename_binding_map(&ownership.binding_to_filename);
        let merged_ref_resolver = MergedRefResolver {
            binding_to_filename: &ownership.binding_to_filename,
            binding_filename_by_atom: &current_binding_filename_by_atom,
            external_imports: &index.external_imports,
            external_import_by_atom: context.external_import_by_atom,
            top_level_decl_indices: &index.decl_indices,
        };
        let mut extra_imports: HashMap<String, Vec<Atom>> = HashMap::default();
        let mut extra_external_imports: HashSet<BindingId> = HashSet::default();
        let mut extra_owned_bindings: HashSet<BindingId> = ownership
            .factory_owned_bindings
            .get(filename)
            .cloned()
            .unwrap_or_default();
        let mut init_bodies: Vec<(Atom, Option<CjsFactoryParams>, Vec<Stmt>)> = Vec::new();

        let already_imported = context
            .module_already_imports
            .get(filename)
            .cloned()
            .unwrap_or_default();

        for mf in factories {
            for write_binding in &mf.write_bindings {
                let owned_binding = index
                    .decl_binding_by_atom
                    .get(&write_binding.0)
                    .unwrap_or(write_binding);
                if ownership
                    .binding_to_filename
                    .get(owned_binding)
                    .is_some_and(|owner| owner == filename)
                    && index.decl_indices.contains_key(owned_binding)
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
                match merged_ref_resolver.resolve(ref_binding, filename) {
                    MergedRefTarget::Import { filename, atom } => {
                        extra_imports.entry(filename).or_default().push(atom);
                    }
                    MergedRefTarget::External(binding) => {
                        extra_external_imports.insert(binding);
                    }
                    MergedRefTarget::Owned => {
                        extra_owned_bindings.insert(ref_binding.clone());
                    }
                    MergedRefTarget::SameModule | MergedRefTarget::Unresolved => {}
                }
            }
            init_bodies.push((mf.var_name, mf.cjs_params, mf.stmts));
        }

        let mut changed = true;
        while changed {
            changed = false;
            let owned_bindings: Vec<BindingId> = extra_owned_bindings.iter().cloned().collect();
            for owned_binding in owned_bindings {
                for ref_binding in index
                    .decl_references
                    .get(&owned_binding)
                    .into_iter()
                    .flatten()
                {
                    if extra_owned_bindings.contains(ref_binding)
                        || already_imported.contains(ref_binding)
                    {
                        continue;
                    }
                    match merged_ref_resolver.resolve(ref_binding, filename) {
                        MergedRefTarget::Import { filename, atom } => {
                            extra_imports.entry(filename).or_default().push(atom);
                        }
                        MergedRefTarget::External(binding) => {
                            extra_external_imports.insert(binding);
                        }
                        MergedRefTarget::Owned => {
                            extra_owned_bindings.insert(ref_binding.clone());
                            changed = true;
                        }
                        MergedRefTarget::SameModule | MergedRefTarget::Unresolved => {}
                    }
                }
            }
        }
        (
            extra_imports,
            extra_external_imports,
            extra_owned_bindings,
            init_bodies,
        )
    };

    // `extra_owned_bindings` is the complete support-declaration closure
    // discovered for the merged factory body. These bindings used to live in
    // the standalone factory file, so move and export them from the scope
    // owner as part of the same relocation.
    for owned_binding in &extra_owned_bindings {
        ownership.own(owned_binding.clone(), filename);
    }

    // Names the module declares once this merge lands.
    let mut local_atoms = context
        .module_local_atoms
        .get(filename)
        .cloned()
        .unwrap_or_default();
    local_atoms.extend(
        ownership
            .binding_to_filename
            .iter()
            .filter(|(_, owner)| owner.as_str() == filename)
            .map(|((atom, _), _)| atom.clone()),
    );
    local_atoms.extend(extra_owned_bindings.iter().map(|(atom, _)| atom.clone()));

    let mut external_imports: Vec<BindingId> = extra_external_imports.into_iter().collect();
    external_imports.sort_by(|a, b| a.0.cmp(&b.0));
    external_imports.retain(|binding| !local_atoms.contains(&binding.0));

    if let Some(referenced_atoms) = context.module_referenced_atoms.get(filename) {
        augment_imports_with_referenced_atoms_for_existing_sources(
            &mut extra_imports,
            filename,
            referenced_atoms,
            context.pre_merge_binding_filename_by_atom,
            Some(&local_atoms),
        );
    }
    let mut source_filenames: Vec<String> = extra_imports.keys().cloned().collect();
    source_filenames.sort();
    let mut named_imports = Vec::new();
    for source_filename in source_filenames {
        let mut names = extra_imports.remove(&source_filename).unwrap_or_default();
        names.retain(|name| !local_atoms.contains(name));
        names.sort();
        names.dedup();
        if names.is_empty() {
            continue;
        }
        named_imports.push((relative_import_path(filename, &source_filename), names));
    }

    let module_factory_owned = ownership.factory_owned_bindings.get(filename);
    let module_local = context.module_local_atoms.get(filename);
    // Reference analysis is binding-granular, so emission must be too.
    // Group first because multiple adopted bindings can share one mixed
    // declaration; filtering each independently and deduping by item index
    // would arbitrarily discard all but one binding.
    let mut owned_atoms_by_index: HashMap<usize, HashSet<Atom>> = HashMap::default();
    for binding in extra_owned_bindings.into_iter().filter(|binding| {
        module_factory_owned.is_some_and(|owned| owned.contains(binding))
            || module_local.is_none_or(|local_atoms| !local_atoms.contains(&binding.0))
    }) {
        if let Some(index) = index.decl_indices.get(&binding) {
            owned_atoms_by_index
                .entry(*index)
                .or_default()
                .insert(binding.0.clone());
            ownership.entry_duplicate_declarations.insert(binding);
        }
    }
    let mut owned_items: Vec<(usize, HashSet<Atom>)> = owned_atoms_by_index.into_iter().collect();
    owned_items.sort_by_key(|(index, _)| *index);

    let mut helper_reserved_atoms = local_atoms;
    helper_reserved_atoms.extend(init_bodies.iter().map(|(name, _, _)| name.clone()));

    MergedModulePlan {
        module_index,
        external_imports,
        named_imports,
        owned_items,
        export_names: factory_owned_export_names(filename, &ownership.factory_owned_bindings),
        init_bodies,
        helper_reserved_atoms,
    }
}

/// Adds to `imports_by_source` the referenced names that resolve to a
/// source file the module already imports from, so one import statement
/// covers them.
pub(super) fn augment_imports_with_referenced_atoms_for_existing_sources(
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

/// Where a merged-factory reference resolves when synthesizing the imports
/// its body needs inside the target module.
enum MergedRefTarget {
    /// Import `atom` from `filename`.
    Import { filename: String, atom: Atom },
    /// Re-materialize this external import declaration.
    External(BindingId),
    /// Declared at the bundle top level; adopt the declaration into the module.
    Owned,
    /// Already lives in the target module — nothing to synthesize.
    SameModule,
    /// Unknown binding — leave it alone.
    Unresolved,
}

/// The binding-resolution cascade shared by the merged-factory import scan
/// and its owned-binding fixed point: exact binding → atom fallback →
/// external import (exact, then atom) → top-level owned declaration.
struct MergedRefResolver<'a> {
    pub(super) binding_to_filename: &'a HashMap<BindingId, String>,
    pub(super) binding_filename_by_atom: &'a HashMap<Atom, (BindingId, String)>,
    pub(super) external_imports: &'a HashMap<BindingId, ExternalImport>,
    pub(super) external_import_by_atom: &'a HashMap<Atom, BindingId>,
    pub(super) top_level_decl_indices: &'a HashMap<BindingId, usize>,
}

impl MergedRefResolver<'_> {
    pub(super) fn resolve(
        &self,
        ref_binding: &BindingId,
        module_filename: &str,
    ) -> MergedRefTarget {
        if let Some(source_filename) = self.binding_to_filename.get(ref_binding) {
            return if source_filename.as_str() == module_filename {
                MergedRefTarget::SameModule
            } else {
                MergedRefTarget::Import {
                    filename: source_filename.clone(),
                    atom: ref_binding.0.clone(),
                }
            };
        }
        if let Some((source_binding, source_filename)) =
            self.binding_filename_by_atom.get(&ref_binding.0)
        {
            return if source_filename.as_str() == module_filename {
                MergedRefTarget::SameModule
            } else {
                MergedRefTarget::Import {
                    filename: source_filename.clone(),
                    atom: source_binding.0.clone(),
                }
            };
        }
        if self.external_imports.contains_key(ref_binding) {
            return MergedRefTarget::External(ref_binding.clone());
        }
        if let Some(import_binding) = self.external_import_by_atom.get(&ref_binding.0) {
            return MergedRefTarget::External(import_binding.clone());
        }
        if self.top_level_decl_indices.contains_key(ref_binding) {
            return MergedRefTarget::Owned;
        }
        MergedRefTarget::Unresolved
    }
}
