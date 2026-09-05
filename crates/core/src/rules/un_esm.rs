use std::collections::{hash_map::Entry, HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent,
    BlockStmt, CallExpr, Callee, CondExpr, Decl, ExportAll, ExportDecl, ExportDefaultExpr,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, ForHead, ForInStmt, Function,
    FunctionBody, Id, Ident, IdentName, IfStmt, ImportDecl, ImportDefaultSpecifier,
    ImportNamedSpecifier, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl,
    ModuleExportName, ModuleItem, NamedExport, ObjectPatProp, OptCall, OptChainBase, Pat, Prop,
    PropName, PropOrSpread, ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt, Str, TaggedTpl,
    ThisExpr, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::{find_pat_ids, ExprFactory};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_id;
use crate::analysis::binding_uses::{BindingId, BindingUseIndex, UseKind};
use crate::facts::{collect_module_facts, ModuleFactsMap};
use crate::js_names::{is_reserved_binding_name, is_valid_identifier_name};
use crate::module_path::resolve_relative_specifier;
use crate::provider_namespace_repair::run_provider_namespace_repair;
use crate::utils::paren::strip_parens;
use crate::utils::prototype_members::is_prototype_mutating_member_name;

use super::decl_utils::{collect_decl_names, collect_pat_names, same_ident};
use super::eval_utils::{
    direct_eval_call_source, js_source_mentions_binding, DirectEvalAnalyzer, DirectEvalPresence,
    EvalCallSource,
};
use super::helper_matcher::count_binding_refs;
use super::rename_utils::{
    collect_module_names, collect_unresolved_reference_names, rename_bindings, BindingRename,
};
use super::RewriteLevel;

pub struct UnEsm {
    unresolved_mark: Mark,
    level: RewriteLevel,
    current_filename: Option<String>,
}

impl UnEsm {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
            current_filename: None,
        }
    }

    pub(crate) fn with_current_filename(mut self, current_filename: Option<&str>) -> Self {
        self.current_filename = current_filename.map(str::to_owned);
        self
    }
}

// ============================================================
// Classification types
// ============================================================

/// Classified CJS require kinds
enum CjsRequireKind {
    /// require('foo') bare statement → import 'foo'
    Bare { source: String },
    /// var foo = require('foo') → import foo from 'foo'
    Default { local: Ident, source: String },
    /// var { a, b: c } = require('foo') → import { a, b as c } from 'foo'
    Named {
        specifiers: Vec<(Atom, Ident)>,
        source: String,
    },
    /// var foo = require('foo').default → import foo from 'foo'
    DefaultProp { local: Ident, source: String },
    /// var foo = require('foo').bar → import { bar as foo } from 'foo'
    NamedProp {
        prop: Atom,
        local: Ident,
        source: String,
    },
}

/// Classified CJS export kinds
enum CjsExportKind {
    /// Object.defineProperty(exports, "__esModule", ...) marker → remove
    EsModuleFlag,
    /// module.exports = expr → export default expr
    ModuleExportsDefault { expr: Box<Expr> },
    /// exports.foo = expr or module.exports.foo = expr
    Named {
        name: Atom,
        expr: Box<Expr>,
        is_void: bool,
    },
    /// exports.default = expr → export default expr
    NamedDefault { expr: Box<Expr> },
    /// `Object.defineProperty(exports, "name", { get: () => dep.member })`
    /// where `dep` is a stable top-level `require("source")` binding.
    ReExport {
        name: Atom,
        imported: Atom,
        source: String,
        binding: BindingId,
    },
    /// exports.default = expr; module.exports = exports.default → keep the real default
    DefaultMirror,
    /// module.exports.default = module.exports pattern → remove
    SelfRef,
}

/// Classification of a module item
enum Classified {
    ExistingImport(ImportDecl),
    CjsRequire(CjsRequireKind),
    CjsExport { span: Span, kind: CjsExportKind },
    Keep(ModuleItem),
}

/// Per-source import accumulator — stores full Ident to preserve SyntaxContext
#[derive(Default)]
struct SourceEntry {
    first_default: Option<Ident>,
    named: Vec<(Atom, Ident)>, // (imported_name, local_ident)
    extra_defaults: Vec<Ident>,
    bare: bool,
    /// Whether any CJS require() was found for this source.
    /// If false, existing import declarations can be passed through unchanged.
    has_cjs: bool,
    /// Original ImportDecl(s) for this source (used when has_cjs=false)
    original_imports: Vec<ImportDecl>,
}

impl SourceEntry {
    fn add_default(&mut self, local: Ident) {
        if self.first_default.is_none() {
            self.first_default = Some(local);
        } else {
            self.extra_defaults.push(local);
        }
    }

    fn add_named(&mut self, imported: Atom, local: Ident) {
        // dedup by local sym
        if !self.named.iter().any(|(_, l)| l.sym == local.sym) {
            self.named.push((imported, local));
        }
    }

    fn set_bare(&mut self) {
        self.bare = true;
    }
}

// ============================================================
// Main implementation
// ============================================================

impl VisitMut for UnEsm {
    fn visit_mut_module(&mut self, module: &mut swc_core::ecma::ast::Module) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        let current_filename = self.current_filename.clone();
        let has_local_self_require =
            contains_local_self_require(module, self.unresolved_mark, current_filename.as_deref());
        // Named/default member argument pre-passes cannot prove that a self
        // export is initialized before the call. Keep the whole CommonJS
        // boundary; otherwise an early partial-object read becomes an ESM TDZ
        // read.
        if has_local_self_require
            && (has_toplevel_require_named_member_self_arg(
                &module.body,
                self.unresolved_mark,
                current_filename.as_deref(),
            ) || has_toplevel_require_default_member_self_arg(
                &module.body,
                self.unresolved_mark,
                current_filename.as_deref(),
            ))
        {
            return;
        }
        let original_cjs_module = if has_local_self_require {
            Some(module.clone())
        } else {
            None
        };
        recover_coupled_commonjs_default_binding(module, self.unresolved_mark);
        // Phase -1: hoist require() calls out of complex expressions
        hoist_embedded_requires(module, self.unresolved_mark);
        // Parallel to the named-member pass inside hoist_embedded_requires.
        // Must not sit behind has_hoistable_require: a file whose only
        // hoistable shape is `require(mod).default` as a call argument would
        // otherwise skip the pre-pass entirely.
        hoist_toplevel_require_default_member_args(module, self.unresolved_mark);
        split_called_module_exports_assignments(module, self.unresolved_mark);
        split_chained_local_module_exports_assignments(module, self.unresolved_mark);
        // Phase 0: split compound `var s = exports.X = expr` →
        //          `var s = expr; exports.X = s;`
        split_compound_exports(module, self.unresolved_mark);
        rewrite_commonjs_export_star_loops(module, self.unresolved_mark);
        rewrite_webpack_export_getters(module, self.unresolved_mark);
        rewrite_recovered_default_only_default_compat_block(module, self.unresolved_mark);
        remove_dead_named_only_default_compat_blocks(module, self.unresolved_mark);
        lower_exported_cjs_requires(module, self.unresolved_mark);
        preserve_written_cjs_require_bindings(module, self.unresolved_mark);
        let unresolved_reference_names =
            collect_unresolved_reference_names(module, self.unresolved_mark);
        let all_declared_names = collect_all_declared_names(module);
        let binding_uses = BindingUseIndex::collect(module);
        let commonjs_read_recovery =
            collect_commonjs_read_recovery_evidence(module, self.unresolved_mark, &binding_uses);
        let require_bindings =
            collect_stable_require_bindings(module, &binding_uses, self.unresolved_mark);

        let items = std::mem::take(&mut module.body);

        // Phase 1: classify
        let mut classified: Vec<Classified> = Vec::with_capacity(items.len());

        let mut stable_default = None;
        for item in items {
            let property_write = stable_default.as_ref().and_then(|binding| {
                preserve_default_property_write(&item, binding, self.unresolved_mark)
            });
            let next_default =
                top_level_module_exports_ident_assignment(&item, self.unresolved_mark).filter(
                    |(span, _)| {
                        Some(*span) == commonjs_read_recovery.stable_default_assignment_span
                    },
                );
            let mut entry = classify_item(item, self.unresolved_mark, &require_bindings);
            if let (
                Some(write),
                Classified::CjsExport {
                    kind: CjsExportKind::Named { expr, is_void, .. },
                    ..
                },
            ) = (property_write, &mut entry)
            {
                if matches!(expr.as_ref(), Expr::Ident(id)
                    if stable_default.as_ref().is_some_and(|binding| same_ident(id, binding)))
                {
                    // Reading the proven stable default again is harmless;
                    // keep the original named binding instead of an alias.
                    classified.push(Classified::Keep(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                        span: DUMMY_SP,
                        expr: write,
                    }))));
                } else {
                    *expr = write;
                    // Even an undefined sentinel writes an observable property.
                    *is_void = false;
                }
            }
            classified.push(entry);
            if let Some((_, binding)) = next_default {
                stable_default = Some(binding);
            }
        }

        // Webpack/Babel interop often emits:
        //   exports.default = value;
        //   module.exports = exports.default;
        // The second assignment only mirrors the CommonJS shape.  If treated as
        // the last default export, it strands the real value as a side-effect.
        remove_default_export_mirrors(&mut classified, self.unresolved_mark);

        // Phase 2: export dedup
        struct ExportEntry {
            classified_idx: usize,
            name: Option<Atom>, // None = default
            is_void: bool,
        }

        let mut export_entries: Vec<ExportEntry> = Vec::new();
        for (idx, c) in classified.iter().enumerate() {
            if let Classified::CjsExport { kind, .. } = c {
                let (name, is_void) = match kind {
                    CjsExportKind::EsModuleFlag => continue,
                    CjsExportKind::ModuleExportsDefault { .. } => (None, false),
                    CjsExportKind::NamedDefault { .. } => (None, false),
                    CjsExportKind::ReExport { name, .. } => {
                        ((name.as_ref() != "default").then(|| name.clone()), false)
                    }
                    CjsExportKind::Named { name, is_void, .. } => {
                        ((name.as_ref() != "default").then(|| name.clone()), *is_void)
                    }
                    CjsExportKind::DefaultMirror => {
                        export_entries.push(ExportEntry {
                            classified_idx: idx,
                            name: None,
                            is_void: true,
                        });
                        continue;
                    }
                    CjsExportKind::SelfRef => {
                        export_entries.push(ExportEntry {
                            classified_idx: idx,
                            name: None,
                            is_void: true,
                        });
                        continue;
                    }
                };
                export_entries.push(ExportEntry {
                    classified_idx: idx,
                    name,
                    is_void,
                });
            }
        }

        // For each unique name, find the last non-void index
        let mut last_real: HashMap<Option<Atom>, usize> = HashMap::new();
        for e in &export_entries {
            if !e.is_void {
                last_real.insert(e.name.clone(), e.classified_idx);
            }
        }

        // Build drop set
        let mut drop_set: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for e in &export_entries {
            if e.is_void {
                drop_set.insert(e.classified_idx);
            } else if let Some(&last_idx) = last_real.get(&e.name) {
                if e.classified_idx != last_idx {
                    drop_set.insert(e.classified_idx);
                }
            }
        }

        // A require binding used exclusively by kept live re-export getters no
        // longer needs a local import. The export-from declaration itself is
        // the module evaluation dependency. If any getter is dropped or the
        // binding has another use, retain the ordinary import.
        let mut kept_reexport_counts: HashMap<BindingId, usize> = HashMap::new();
        for (idx, item) in classified.iter().enumerate() {
            if drop_set.contains(&idx) {
                continue;
            }
            if let Classified::CjsExport {
                kind: CjsExportKind::ReExport { binding, .. },
                ..
            } = item
            {
                *kept_reexport_counts.entry(binding.clone()).or_default() += 1;
            }
        }
        let consumed_reexport_bindings: HashSet<BindingId> = kept_reexport_counts
            .into_iter()
            .filter_map(|(binding, count)| {
                (binding_uses.use_count(&binding) == count).then_some(binding)
            })
            .collect();

        // Phase 3: collect imports — build source_map keyed by String
        let mut source_order: Vec<String> = Vec::new();
        let mut source_map: HashMap<String, SourceEntry> = HashMap::new();

        // First pass: mark which sources have CJS requires
        let mut cjs_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
        for c in classified.iter() {
            let src = match c {
                Classified::CjsRequire(CjsRequireKind::Bare { source }) => source.clone(),
                Classified::CjsRequire(CjsRequireKind::Default { local, source }) => {
                    if consumed_reexport_bindings.contains(&(local.sym.clone(), local.ctxt)) {
                        continue;
                    }
                    source.clone()
                }
                Classified::CjsRequire(CjsRequireKind::Named { source, .. }) => source.clone(),
                Classified::CjsRequire(CjsRequireKind::DefaultProp { source, .. }) => {
                    source.clone()
                }
                Classified::CjsRequire(CjsRequireKind::NamedProp { source, .. }) => source.clone(),
                _ => continue,
            };
            cjs_sources.insert(src);
        }

        for c in classified.iter() {
            match c {
                Classified::ExistingImport(import) => {
                    let src = wtf8_to_string(&import.src.value);

                    if cjs_sources.contains(&src) {
                        // Source has CJS requires → absorb non-namespace specifiers into source_map
                        let has_ns = import
                            .specifiers
                            .iter()
                            .any(|s| matches!(s, ImportSpecifier::Namespace(_)));
                        let has_non_ns = import
                            .specifiers
                            .iter()
                            .any(|s| !matches!(s, ImportSpecifier::Namespace(_)));

                        if has_non_ns {
                            let entry =
                                get_or_insert(&mut source_order, &mut source_map, src.clone());
                            entry.has_cjs = true;
                            for spec in &import.specifiers {
                                match spec {
                                    ImportSpecifier::Default(d) => {
                                        entry.add_default(d.local.clone())
                                    }
                                    ImportSpecifier::Named(n) => {
                                        let imported: Atom = match &n.imported {
                                            Some(ModuleExportName::Ident(i)) => i.sym.clone(),
                                            Some(ModuleExportName::Str(_)) => n.local.sym.clone(),
                                            None => n.local.sym.clone(),
                                        };
                                        entry.add_named(imported, n.local.clone());
                                    }
                                    ImportSpecifier::Namespace(_) => {}
                                }
                            }
                        } else if !has_ns && import.specifiers.is_empty() {
                            let entry =
                                get_or_insert(&mut source_order, &mut source_map, src.clone());
                            entry.has_cjs = true;
                            entry.set_bare();
                        }

                        // Namespace specifiers in a source-with-CJS: keep as original pass-through
                        if has_ns {
                            // Build a namespace-only import to pass through
                            let ns_specs: Vec<ImportSpecifier> = import
                                .specifiers
                                .iter()
                                .filter(|s| matches!(s, ImportSpecifier::Namespace(_)))
                                .cloned()
                                .collect();
                            if !ns_specs.is_empty() {
                                let ns_import = ImportDecl {
                                    specifiers: ns_specs,
                                    ..import.clone()
                                };
                                // Use a unique key to preserve ordering in source_order
                                let ns_key = format!("__ns__:{}", src);
                                let entry =
                                    get_or_insert(&mut source_order, &mut source_map, ns_key);
                                entry.original_imports.push(ns_import);
                            }
                        }
                    } else {
                        // No CJS for this source — pass through entire import unchanged
                        let entry = get_or_insert(&mut source_order, &mut source_map, src);
                        entry.original_imports.push(import.clone());
                    }
                }
                Classified::CjsRequire(kind) => match kind {
                    CjsRequireKind::Bare { source } => {
                        let entry =
                            get_or_insert(&mut source_order, &mut source_map, source.clone());
                        entry.has_cjs = true;
                        entry.set_bare();
                    }
                    CjsRequireKind::Default { local, source } => {
                        if consumed_reexport_bindings.contains(&(local.sym.clone(), local.ctxt)) {
                            continue;
                        }
                        let entry =
                            get_or_insert(&mut source_order, &mut source_map, source.clone());
                        entry.has_cjs = true;
                        entry.add_default(local.clone());
                    }
                    CjsRequireKind::Named { specifiers, source } => {
                        let entry =
                            get_or_insert(&mut source_order, &mut source_map, source.clone());
                        entry.has_cjs = true;
                        for (imported, local) in specifiers {
                            entry.add_named(imported.clone(), local.clone());
                        }
                    }
                    CjsRequireKind::DefaultProp { local, source } => {
                        let entry =
                            get_or_insert(&mut source_order, &mut source_map, source.clone());
                        entry.has_cjs = true;
                        entry.add_default(local.clone());
                    }
                    CjsRequireKind::NamedProp {
                        prop,
                        local,
                        source,
                    } => {
                        let entry =
                            get_or_insert(&mut source_order, &mut source_map, source.clone());
                        entry.has_cjs = true;
                        entry.add_named(prop.clone(), local.clone());
                    }
                },
                _ => {}
            }
        }

        // Build import declarations
        let mut import_decls: Vec<ModuleItem> = Vec::new();

        // Process sources in first-seen order
        for src in &source_order {
            let entry = &source_map[src];
            if entry.has_cjs {
                // Merge CJS requires with any existing imports for this source
                build_import_decls(src, entry, &mut import_decls);
            } else {
                // No CJS requires — pass through original imports unchanged
                for orig in &entry.original_imports {
                    import_decls.push(ModuleItem::ModuleDecl(ModuleDecl::Import(orig.clone())));
                }
            }
        }

        // Collect local names that conflict with export names. Export names
        // take priority (they're meaningful from the original source), so we
        // rename the conflicting locals to free up the name for the export.
        let mut local_names: HashSet<Atom> = HashSet::new();
        for item in &import_decls {
            if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
                for spec in &import.specifiers {
                    match spec {
                        ImportSpecifier::Named(n) => {
                            local_names.insert(n.local.sym.clone());
                        }
                        ImportSpecifier::Default(d) => {
                            local_names.insert(d.local.sym.clone());
                        }
                        ImportSpecifier::Namespace(ns) => {
                            local_names.insert(ns.local.sym.clone());
                        }
                    }
                }
            }
        }
        for c in &classified {
            if let Classified::Keep(ModuleItem::Stmt(Stmt::Decl(decl))) = c {
                collect_decl_names(decl, &mut local_names);
            }
        }

        // Find export names that clash with existing locals.
        // Export names take priority (meaningful from original source), so
        // rename the conflicting locals before building export items.
        let mut export_names: HashSet<Atom> = HashSet::new();
        for (idx, c) in classified.iter().enumerate() {
            if drop_set.contains(&idx) {
                continue;
            }
            if let Classified::CjsExport {
                kind:
                    CjsExportKind::Named {
                        name,
                        expr,
                        is_void: false,
                    },
                ..
            } = c
            {
                let is_ident = matches!(expr.as_ref(), Expr::Ident(_));
                if !is_ident && local_names.contains(name) {
                    export_names.insert(name.clone());
                }
            }
        }

        // Rename conflicting locals before building exports. The export
        // expression can reference a conflicting module-level local, so apply
        // binding-id renames to both kept items and export expressions.
        let mut used_export_binding_names = all_declared_names.clone();
        used_export_binding_names.extend(unresolved_reference_names.iter().cloned());
        if !export_names.is_empty() {
            let mut used_names = all_declared_names.clone();
            used_names.extend(export_names.iter().cloned());
            let mut renames = Vec::new();

            collect_conflicting_import_renames(
                &import_decls,
                &export_names,
                &mut used_names,
                &mut renames,
            );
            for c in &classified {
                if let Classified::Keep(ModuleItem::Stmt(Stmt::Decl(decl))) = c {
                    collect_conflicting_decl_renames(
                        decl,
                        &export_names,
                        &mut used_names,
                        &mut renames,
                    );
                }
            }

            if !renames.is_empty() {
                for item in &mut import_decls {
                    rename_bindings(item, &renames);
                }
                for c in classified.iter_mut() {
                    match c {
                        Classified::Keep(item) => rename_bindings(item, &renames),
                        Classified::CjsExport { kind, .. } => rename_export_kind(kind, &renames),
                        _ => {}
                    }
                }
            }
            used_export_binding_names.extend(renames.into_iter().map(|rename| rename.new));
        }

        for item in &classified {
            if let Classified::CjsExport {
                kind: CjsExportKind::Named { name, .. },
                ..
            } = item
            {
                used_export_binding_names.insert(name.clone());
            }
        }

        // Build final module body
        let mut new_body: Vec<ModuleItem> = import_decls;

        for (idx, c) in classified.into_iter().enumerate() {
            match c {
                Classified::ExistingImport(_) => {} // skip, already absorbed
                Classified::CjsRequire(_) => {}     // skip, replaced by import
                Classified::CjsExport { span, kind } => {
                    if drop_set.contains(&idx) {
                        new_body.extend(build_dropped_export_side_effect_items(span, kind));
                    } else {
                        new_body.extend(build_export_items(
                            span,
                            kind,
                            &mut used_export_binding_names,
                            &unresolved_reference_names,
                        ));
                    }
                }
                Classified::Keep(item) => {
                    new_body.push(item);
                }
            }
        }

        recover_stable_commonjs_reads(&mut new_body, self.unresolved_mark, &commonjs_read_recovery);
        merge_decl_and_named_export(&mut new_body);
        inline_adjacent_default_export_aliases(&mut new_body);
        module.body = new_body;

        if let (Some(original), Some(current_filename)) =
            (original_cjs_module, current_filename.as_deref())
        {
            // A self-require used only through a provider's proven named
            // surface can use the same conservative namespace representation
            // as the unpack fact barrier. Apply that proof locally so the
            // phase-1 fact collector sees a linkable edge too. Whole-value,
            // mutable, computed, escaping, and otherwise incompatible reads
            // remain default imports and trigger the rollback below.
            let mut local_facts = ModuleFactsMap::new();
            local_facts.insert(current_filename, collect_module_facts(module));
            run_provider_namespace_repair(
                module,
                &local_facts,
                Some(current_filename),
                self.unresolved_mark,
            );
            if has_unlinkable_default_self_import(module, current_filename) {
                // CommonJS returns the current, partially initialized
                // `module.exports` object for a self-require. If recovery did
                // not also establish a default export, the synthesized default
                // self-import cannot even link. Roll back this rule as one
                // unit instead of mixing an unrepresentable edge with ESM.
                *module = original;
            }
        }
    }
}

pub(crate) fn contains_local_self_require(
    module: &Module,
    unresolved_mark: Mark,
    current_filename: Option<&str>,
) -> bool {
    let Some(current_filename) = current_filename else {
        return false;
    };
    let Some((normalized_filename, current_key)) = current_module_path_context(current_filename)
    else {
        return false;
    };

    let mut finder = LocalSelfRequireFinder {
        unresolved_mark,
        current_filename: &normalized_filename,
        current_key: &current_key,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

fn current_module_path_context(current_filename: &str) -> Option<(String, String)> {
    let normalized_filename = current_filename.replace('\\', "/");
    let basename = normalized_filename.rsplit('/').next()?;
    if basename.is_empty() {
        return None;
    }
    let current_key = resolve_relative_specifier(&normalized_filename, &format!("./{basename}"))?;
    Some((normalized_filename, current_key))
}

fn has_unlinkable_default_self_import(module: &Module, current_filename: &str) -> bool {
    if module_has_default_export(module) {
        return false;
    }
    let Some((normalized_filename, current_key)) = current_module_path_context(current_filename)
    else {
        return false;
    };

    module.body.iter().any(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            return false;
        };
        if !import
            .specifiers
            .iter()
            .any(|specifier| matches!(specifier, ImportSpecifier::Default(_)))
        {
            return false;
        }
        let Some(source) = import.src.value.as_str() else {
            return false;
        };
        resolve_relative_specifier(&normalized_filename, source).as_deref() == Some(&current_key)
    })
}

fn module_has_default_export(module: &Module) -> bool {
    module.body.iter().any(|item| {
        let ModuleItem::ModuleDecl(decl) = item else {
            return false;
        };
        match decl {
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => true,
            ModuleDecl::ExportNamed(export) => {
                export.specifiers.iter().any(|specifier| match specifier {
                    ExportSpecifier::Default(_) => true,
                    ExportSpecifier::Named(named) => module_export_name_is_default(
                        named.exported.as_ref().unwrap_or(&named.orig),
                    ),
                    ExportSpecifier::Namespace(namespace) => {
                        module_export_name_is_default(&namespace.name)
                    }
                })
            }
            _ => false,
        }
    })
}

fn module_export_name_is_default(name: &ModuleExportName) -> bool {
    match name {
        ModuleExportName::Ident(name) => name.sym == "default",
        ModuleExportName::Str(name) => name.value.as_str() == Some("default"),
    }
}

struct LocalSelfRequireFinder<'a> {
    unresolved_mark: Mark,
    current_filename: &'a str,
    current_key: &'a str,
    found: bool,
}

impl Visit for LocalSelfRequireFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if self.found {
            return;
        }
        if let Some(source) = is_require_call(call, self.unresolved_mark) {
            if resolve_relative_specifier(self.current_filename, &source).as_deref()
                == Some(self.current_key)
            {
                self.found = true;
                return;
            }
        }
        call.visit_children_with(self);
    }
}

/// Evidence retained across CommonJS export classification. The classifier
/// removes the runtime assignment nodes that establish these identities, so
/// read recovery must prove them before building the ESM body and consume the
/// proof afterwards.
#[derive(Default)]
struct CommonJsReadRecoveryEvidence {
    /// Span of the sole direct `module.exports = stableBinding` assignment.
    /// The rebuilt `export default stableBinding` keeps this span, allowing us
    /// to recover the possibly-renamed binding without carrying stale ids.
    stable_default_assignment_span: Option<Span>,
    /// Static `exports.name` properties with one stable value assignment. The
    /// rebuilt body proves the replacement binding and, for direct calls, that
    /// the function cannot observe a changed receiver.
    stable_named_properties: HashSet<Atom>,
}

fn collect_commonjs_read_recovery_evidence(
    module: &Module,
    unresolved_mark: Mark,
    uses: &BindingUseIndex,
) -> CommonJsReadRecoveryEvidence {
    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        // Direct eval can read or replace CommonJS runtime properties and
        // local captures without producing statically visible use sites.
        return CommonJsReadRecoveryEvidence::default();
    }

    CommonJsReadRecoveryEvidence {
        stable_default_assignment_span: collect_stable_default_assignment_span(
            module,
            unresolved_mark,
            uses,
        ),
        stable_named_properties: collect_stable_named_properties(module, unresolved_mark, uses),
    }
}

/// Recover Babel runtime helpers whose mutable CommonJS value is always kept
/// identical to one top-level function binding:
///
/// ```js
/// function helper(value) {
///   module.exports = helper = selectImplementation();
///   module.exports.default = module.exports;
///   return helper(value);
/// }
/// module.exports = helper;
/// module.exports.default = module.exports;
/// ```
///
/// A snapshot `export default helper` is insufficient here: the helper and
/// `module.exports` are replaced together on first use. When the complete
/// resolved module proves that every write remains coupled, use the function
/// binding as the runtime value and expose it through a live export specifier.
/// Property mirrors stay as real mutations of that binding.
fn recover_coupled_commonjs_default_binding(module: &mut Module, unresolved_mark: Mark) {
    let Some(plan) = find_coupled_commonjs_default_plan(module, unresolved_mark) else {
        return;
    };

    // UnAssignmentMerging may have expanded a simple
    // `module.exports = helper = value` into two adjacent assignments. Prove
    // against a normalized clone so a failed proof never mutates the input.
    let mut proof_module = module.clone();
    if !merge_candidate_split_assignments(&mut proof_module, &plan.binding, unresolved_mark)
        || !prove_coupled_commonjs_default_plan(&proof_module, &plan, unresolved_mark)
    {
        return;
    }

    let merged = merge_candidate_split_assignments(module, &plan.binding, unresolved_mark);
    debug_assert!(
        merged,
        "the proven candidate function must remain available"
    );

    module.body[plan.assignment_index] =
        make_live_default_export_item(plan.assignment_span, plan.binding.clone());
    module.visit_mut_with(&mut CoupledCommonJsDefaultRewriter {
        unresolved_mark,
        binding: plan.binding,
    });
}

struct CoupledCommonJsDefaultPlan {
    binding: Ident,
    function_index: usize,
    assignment_index: usize,
    assignment_span: Span,
}

fn find_coupled_commonjs_default_plan(
    module: &Module,
    unresolved_mark: Mark,
) -> Option<CoupledCommonJsDefaultPlan> {
    // Do not combine an authored/recovered ESM surface with this CommonJS
    // lifetime proof. Imports are harmless and still go through normal UnEsm
    // source coalescing afterwards.
    if module.body.iter().any(|item| {
        matches!(
            item,
            ModuleItem::ModuleDecl(decl) if !matches!(decl, ModuleDecl::Import(_))
        )
    }) {
        return None;
    }

    let assignments: Vec<_> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            top_level_module_exports_ident_assignment(item, unresolved_mark)
                .map(|(span, binding)| (index, span, binding))
        })
        .collect();
    let [(assignment_index, assignment_span, binding)] = assignments.as_slice() else {
        return None;
    };

    let functions: Vec<_> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = item else {
                return None;
            };
            (binding_id(&function.ident) == binding_id(binding)).then_some(index)
        })
        .collect();
    let [function_index] = functions.as_slice() else {
        return None;
    };
    if function_index >= assignment_index {
        return None;
    }

    let self_refs: Vec<_> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            is_module_exports_self_ref_item(item, unresolved_mark).then_some(index)
        })
        .collect();
    let [self_ref_index] = self_refs.as_slice() else {
        return None;
    };
    if self_ref_index <= assignment_index {
        return None;
    }

    // Calling or exposing the hoisted helper before the CommonJS assignment
    // could observe the initial `{}` value instead of the candidate binding.
    let binding_key = binding_id(binding);
    if module.body[..*assignment_index]
        .iter()
        .enumerate()
        .any(|(index, item)| {
            index != *function_index && count_binding_refs(item, &binding_key) != 0
        })
    {
        return None;
    }

    // Outside the exact top-level assignment, mirror, and candidate function,
    // no statement may observe the CommonJS module object. This excludes a
    // second deferred writer, an early read, or an alias hidden in another
    // closure without needing control-flow inference.
    if module.body.iter().enumerate().any(|(index, item)| {
        index != *function_index
            && index != *assignment_index
            && index != *self_ref_index
            && item_has_unresolved_binding(item, "module", unresolved_mark)
    }) {
        return None;
    }

    Some(CoupledCommonJsDefaultPlan {
        binding: binding.clone(),
        function_index: *function_index,
        assignment_index: *assignment_index,
        assignment_span: *assignment_span,
    })
}

fn top_level_module_exports_ident_assignment(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(Span, Ident)> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Assign(assignment) = strip_parens(&statement.expr) else {
        return None;
    };
    if assignment.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
        return None;
    };
    if !is_module_exports_member(target, unresolved_mark) {
        return None;
    }
    let Expr::Ident(binding) = strip_parens(&assignment.right) else {
        return None;
    };
    Some((statement.span, binding.clone()))
}

fn is_module_exports_self_ref_item(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return false;
    };
    let Expr::Assign(assignment) = strip_parens(&statement.expr) else {
        return false;
    };
    is_module_exports_self_ref_assignment(assignment, unresolved_mark)
}

fn is_module_exports_self_ref_assignment(assignment: &AssignExpr, unresolved_mark: Mark) -> bool {
    if assignment.op != AssignOp::Assign
        || !is_module_exports_expr(strip_parens(&assignment.right), unresolved_mark)
    {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
        return false;
    };
    is_module_exports_expr(target.obj.as_ref(), unresolved_mark)
        && is_ident_prop(&target.prop).is_some_and(|property| property.as_ref() == "default")
}

fn item_has_unresolved_binding(item: &ModuleItem, name: &str, unresolved_mark: Mark) -> bool {
    let mut collector = UnresolvedBindingIdCollector::new(name, unresolved_mark);
    item.visit_with(&mut collector);
    !collector.ids.is_empty()
}

fn merge_candidate_split_assignments(
    module: &mut Module,
    binding: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let binding_key = binding_id(binding);
    let mut found = false;
    for item in &mut module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = item else {
            continue;
        };
        if binding_id(&function.ident) != binding_key {
            continue;
        }
        let Some(body) = &mut function.function.body else {
            return false;
        };
        let mut merger = CoupledSplitAssignmentMerger {
            unresolved_mark,
            binding: &binding_key,
        };
        merger.visit_mut_stmts(&mut body.stmts);
        found = true;
    }
    found
}

struct CoupledSplitAssignmentMerger<'a> {
    unresolved_mark: Mark,
    binding: &'a BindingId,
}

impl VisitMut for CoupledSplitAssignmentMerger<'_> {
    fn visit_mut_function(&mut self, _: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}

    fn visit_mut_stmts(&mut self, statements: &mut Vec<Stmt>) {
        statements.visit_mut_children_with(self);

        let mut index = 0usize;
        while index + 1 < statements.len() {
            let Some(merged) = merge_split_coupled_assignment_pair(
                &statements[index],
                &statements[index + 1],
                self.binding,
                self.unresolved_mark,
            ) else {
                index += 1;
                continue;
            };
            let span = match &statements[index] {
                Stmt::Expr(statement) => statement.span,
                _ => unreachable!("the pair matcher accepts only expression statements"),
            };
            statements[index] = Stmt::Expr(ExprStmt {
                span,
                expr: Box::new(Expr::Assign(merged)),
            });
            statements.remove(index + 1);
        }
    }
}

fn merge_split_coupled_assignment_pair(
    first: &Stmt,
    second: &Stmt,
    binding: &BindingId,
    unresolved_mark: Mark,
) -> Option<AssignExpr> {
    let Stmt::Expr(first_statement) = first else {
        return None;
    };
    let Expr::Assign(module_assignment) = strip_parens(&first_statement.expr) else {
        return None;
    };
    if module_assignment.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &module_assignment.left else {
        return None;
    };
    if !is_module_exports_member(target, unresolved_mark) {
        return None;
    }

    let Stmt::Expr(second_statement) = second else {
        return None;
    };
    let Expr::Assign(binding_assignment) = strip_parens(&second_statement.expr) else {
        return None;
    };
    if binding_assignment.op != AssignOp::Assign
        || !assign_target_matches_binding(&binding_assignment.left, binding)
        || !same_split_coupled_value(&module_assignment.right, &binding_assignment.right)
    {
        return None;
    }

    let mut merged = module_assignment.clone();
    merged.right = Box::new(Expr::Assign(binding_assignment.clone()));
    Some(merged)
}

fn same_split_coupled_value(left: &Expr, right: &Expr) -> bool {
    matches!(
        (strip_parens(left), strip_parens(right)),
        (Expr::Ident(left), Expr::Ident(right))
            if binding_id(left) == binding_id(right)
    )
}

fn assign_target_matches_binding(target: &AssignTarget, binding: &BindingId) -> bool {
    matches!(
        target,
        AssignTarget::Simple(SimpleAssignTarget::Ident(candidate))
            if binding_id(&candidate.id) == *binding
    )
}

fn prove_coupled_commonjs_default_plan(
    module: &Module,
    plan: &CoupledCommonJsDefaultPlan,
    unresolved_mark: Mark,
) -> bool {
    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        return false;
    }

    let mut unresolved_exports = UnresolvedBindingIdCollector::new("exports", unresolved_mark);
    module.visit_with(&mut unresolved_exports);
    if !unresolved_exports.ids.is_empty() {
        return false;
    }

    let mut receiver_sensitive = ReceiverSensitiveModuleExportsCall {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut receiver_sensitive);
    if receiver_sensitive.found {
        return false;
    }

    let Some(function) = module.body.get(plan.function_index).and_then(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = item else {
            return None;
        };
        (binding_id(&function.ident) == binding_id(&plan.binding)).then_some(function)
    }) else {
        return false;
    };
    let Some(body) = &function.function.body else {
        return false;
    };
    let binding_key = binding_id(&plan.binding);
    let mut writes = CoupledCommonJsWriteProof {
        unresolved_mark,
        binding: &binding_key,
        recognized_module_writes: 0,
        recognized_binding_writes: 0,
        saw_nested_self_ref: false,
        valid: true,
    };
    body.stmts.visit_with(&mut writes);
    if !writes.valid
        || !writes.saw_nested_self_ref
        || writes.recognized_module_writes == 0
        || writes.recognized_module_writes != writes.recognized_binding_writes
    {
        return false;
    }

    let uses = BindingUseIndex::collect(module);
    let binding_writes = uses
        .use_sites(&binding_key)
        .iter()
        .filter(|site| matches!(site.kind, UseKind::Write | UseKind::ReadWrite))
        .count();
    if binding_writes != writes.recognized_binding_writes {
        return false;
    }

    let mut unresolved_module = UnresolvedBindingIdCollector::new("module", unresolved_mark);
    module.visit_with(&mut unresolved_module);
    let mut whole_value_writes = 0usize;
    for module_binding in &unresolved_module.ids {
        for site in uses.use_sites(module_binding) {
            match &site.kind {
                UseKind::StaticMemberRead(property) if property.as_ref() == "exports" => {}
                UseKind::StaticMemberWrite(property) if property.as_ref() == "exports" => {
                    whole_value_writes += 1;
                }
                _ => return false,
            }
        }
    }

    // One whole-value write is the direct top-level `module.exports = helper`;
    // every remaining write must be paired inside the helper body.
    whole_value_writes == writes.recognized_module_writes + 1
}

struct CoupledCommonJsWriteProof<'a> {
    unresolved_mark: Mark,
    binding: &'a BindingId,
    recognized_module_writes: usize,
    recognized_binding_writes: usize,
    saw_nested_self_ref: bool,
    valid: bool,
}

impl Visit for CoupledCommonJsWriteProof<'_> {
    fn visit_opt_chain_expr(&mut self, chain: &swc_core::ecma::ast::OptChainExpr) {
        if !self.valid {
            return;
        }
        // The rewriter substitutes only plain `module.exports` members; an
        // optional-chained access would survive as an orphaned free `module`.
        if let OptChainBase::Member(member) = chain.base.as_ref() {
            if let Expr::Ident(object) = strip_parens(&member.obj) {
                if is_unresolved_ident(object, "module", self.unresolved_mark) {
                    self.valid = false;
                    return;
                }
            }
        }
        chain.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        if !self.valid {
            return;
        }
        if is_module_exports_self_ref_assignment(assignment, self.unresolved_mark) {
            self.saw_nested_self_ref = true;
            assignment.visit_children_with(self);
            return;
        }

        if matches!(
            &assignment.left,
            AssignTarget::Simple(SimpleAssignTarget::Member(target))
                if is_module_exports_member(target, self.unresolved_mark)
        ) {
            // A compound write (`*=`, `||=`, ...) reads or conditionally
            // skips the right side; deleting it is not a coupled rewrite.
            if assignment.op != AssignOp::Assign {
                self.valid = false;
                return;
            }
            let Expr::Assign(binding_assignment) = strip_parens(&assignment.right) else {
                self.valid = false;
                return;
            };
            if binding_assignment.op != AssignOp::Assign
                || !assign_target_matches_binding(&binding_assignment.left, self.binding)
            {
                self.valid = false;
                return;
            }
            self.recognized_module_writes += 1;
            self.recognized_binding_writes += 1;
            binding_assignment.right.visit_with(self);
            return;
        }

        if assign_target_matches_binding(&assignment.left, self.binding) {
            self.valid = false;
            return;
        }
        assignment.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

struct ReceiverSensitiveModuleExportsCall {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for ReceiverSensitiveModuleExportsCall {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if matches!(
            &call.callee,
            Callee::Expr(callee)
                if is_module_exports_expr(strip_parens(callee), self.unresolved_mark)
        ) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        if is_module_exports_expr(strip_parens(&call.callee), self.unresolved_mark) {
            self.found = true;
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_tagged_tpl(&mut self, tagged: &TaggedTpl) {
        if is_module_exports_expr(strip_parens(&tagged.tag), self.unresolved_mark) {
            self.found = true;
            return;
        }
        tagged.visit_children_with(self);
    }
}

fn make_live_default_export_item(span: Span, mut binding: Ident) -> ModuleItem {
    binding.span = DUMMY_SP;
    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
        span,
        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
            span: DUMMY_SP,
            orig: ModuleExportName::Ident(binding),
            exported: Some(ModuleExportName::Ident(
                IdentName::new("default".into(), DUMMY_SP).into(),
            )),
            is_type_only: false,
        })],
        src: None,
        type_only: false,
        with: None,
    }))
}

struct CoupledCommonJsDefaultRewriter {
    unresolved_mark: Mark,
    binding: Ident,
}

impl VisitMut for CoupledCommonJsDefaultRewriter {
    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if let Expr::Assign(assignment) = expression {
            let is_whole_value_write = assignment.op == AssignOp::Assign
                && matches!(
                    &assignment.left,
                    AssignTarget::Simple(SimpleAssignTarget::Member(target))
                        if is_module_exports_member(target, self.unresolved_mark)
                );
            if is_whole_value_write {
                let Expr::Assign(binding_assignment) = strip_parens(&assignment.right) else {
                    debug_assert!(false, "the proof accepted an uncoupled CommonJS write");
                    return;
                };
                debug_assert!(assign_target_matches_binding(
                    &binding_assignment.left,
                    &binding_id(&self.binding),
                ));
                *expression = Expr::Assign(binding_assignment.clone());
                expression.visit_mut_children_with(self);
                return;
            }
        }

        expression.visit_mut_children_with(self);
        if is_module_exports_expr(expression, self.unresolved_mark) {
            let span = match expression {
                Expr::Member(member) => member.span,
                _ => DUMMY_SP,
            };
            let mut binding = self.binding.clone();
            binding.span = span;
            *expression = Expr::Ident(binding);
        }
    }
}

fn collect_stable_default_assignment_span(
    module: &Module,
    unresolved_mark: Mark,
    uses: &BindingUseIndex,
) -> Option<Span> {
    let mut direct = Vec::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            continue;
        };
        let Expr::Assign(assignment) = strip_parens(&statement.expr) else {
            continue;
        };
        if assignment.op != AssignOp::Assign {
            continue;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
            continue;
        };
        if !is_module_exports_member(target, unresolved_mark) {
            continue;
        }
        let Expr::Ident(module_ident) = target.obj.as_ref() else {
            continue;
        };
        let Expr::Ident(value) = strip_parens(&assignment.right) else {
            continue;
        };
        direct.push((
            statement.span,
            (module_ident.sym.clone(), module_ident.ctxt),
            (value.sym.clone(), value.ctxt),
        ));
    }
    let [(span, module_binding, value_binding)] = direct.as_slice() else {
        return None;
    };

    // Every access to `module` must be either the `exports` property itself or
    // a harmless existence probe, and exactly one access may replace that
    // property. This catches conditional/deferred second assignments, direct
    // rebinding, delete/update, and reflective uses without flow analysis.
    let mut whole_value_writes = 0usize;
    for site in uses.use_sites(module_binding) {
        match &site.kind {
            UseKind::StaticMemberRead(property) if property.as_ref() == "exports" => {}
            UseKind::StaticMemberWrite(property) if property.as_ref() == "exports" => {
                whole_value_writes += 1;
            }
            UseKind::TypeofOperand => {}
            _ => return None,
        }
    }
    if whole_value_writes != 1
        || !uses.has_declaration(value_binding)
        || uses.has_direct_write(value_binding)
    {
        return None;
    }

    Some(*span)
}

/// A named ESM export alone does not preserve a property on the default
/// object. Keep that write as the export initializer, so aliases still observe
/// it and its RHS/setter run once at the original position. The caller only
/// enables this after the existing whole-module stable-default proof succeeds;
/// writes to the initial `exports` object are a different lifetime.
fn preserve_default_property_write(
    item: &ModuleItem,
    binding: &Ident,
    unresolved_mark: Mark,
) -> Option<Box<Expr>> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Assign(assignment) = strip_parens(&statement.expr) else {
        return None;
    };
    if assignment.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assignment.left else {
        return None;
    };
    if !is_module_exports_expr(strip_parens(&member.obj), unresolved_mark) {
        return None;
    }
    let mut assignment = assignment.clone();
    let mut member = member.clone();
    member.obj = Box::new(Expr::Ident(binding.clone()));
    assignment.left = AssignTarget::Simple(SimpleAssignTarget::Member(member));
    Some(Box::new(Expr::Assign(assignment)))
}

fn collect_stable_named_properties(
    module: &Module,
    unresolved_mark: Mark,
    uses: &BindingUseIndex,
) -> HashSet<Atom> {
    #[derive(Default)]
    struct DirectWrites {
        count: usize,
        value_writes: usize,
        saw_value: bool,
        undefined_after_value: bool,
    }

    let mut direct_writes: HashMap<Atom, DirectWrites> = HashMap::new();
    let mut exports_bindings = HashSet::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
            continue;
        };
        let Expr::Assign(assignment) = strip_parens(&statement.expr) else {
            continue;
        };
        if assignment.op != AssignOp::Assign {
            continue;
        }
        let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assignment.left else {
            continue;
        };
        let Expr::Ident(exports) = target.obj.as_ref() else {
            continue;
        };
        if !is_unresolved_ident(exports, "exports", unresolved_mark) {
            continue;
        }
        let Some(property) = is_ident_prop(&target.prop) else {
            continue;
        };
        if property.as_ref() == "default" {
            continue;
        }
        exports_bindings.insert((exports.sym.clone(), exports.ctxt));
        let writes = direct_writes.entry(property).or_default();
        writes.count += 1;
        if is_void_or_undefined(&assignment.right, unresolved_mark) {
            writes.undefined_after_value |= writes.saw_value;
        } else {
            writes.value_writes += 1;
            writes.saw_value = true;
        }
    }
    if exports_bindings.len() != 1 {
        return HashSet::new();
    }
    let exports_binding = exports_bindings
        .iter()
        .next()
        .expect("the length check proves one exports binding");

    // A bare escape/rebinding of `exports`, a computed access, or a delete
    // could change any named property. Keep the proof deliberately local to
    // modules whose complete runtime surface is static member access.
    let mut write_counts: HashMap<Atom, usize> = HashMap::new();
    for site in uses.use_sites(exports_binding) {
        match &site.kind {
            UseKind::StaticMemberRead(property) | UseKind::StaticMemberWrite(property)
                if is_prototype_mutating_member_name(property.as_ref()) =>
            {
                // `exports.__defineGetter__(...)` can redefine any proven
                // property as an accessor, and a `__proto__` write changes
                // lookup for names without an own write. Neither surfaces as
                // a write of the affected name, so the whole proof fails.
                return HashSet::new();
            }
            UseKind::StaticMemberRead(_) | UseKind::TypeofOperand => {}
            UseKind::StaticMemberWrite(property) => {
                *write_counts.entry(property.clone()).or_default() += 1;
            }
            _ => return HashSet::new(),
        }
    }

    // `module.exports.name` aliases the initial `exports` object only until a
    // whole-value replacement. Rather than reason about that lifetime here,
    // accept named recovery only when `module` is absent (apart from `typeof`).
    let mut module_ids = UnresolvedBindingIdCollector::new("module", unresolved_mark);
    module.visit_with(&mut module_ids);
    if module_ids.ids.iter().any(|binding| {
        uses.use_sites(binding)
            .iter()
            .any(|site| !matches!(site.kind, UseKind::TypeofOperand))
    }) {
        return HashSet::new();
    }

    direct_writes
        .into_iter()
        .filter_map(|(property, direct)| {
            // TypeScript commonly emits `exports.name = void 0` before the
            // real assignment. Those sentinels may precede one stable value,
            // but every write must still be a direct top-level statement and
            // no undefined reset may follow the value.
            (direct.value_writes == 1
                && !direct.undefined_after_value
                && write_counts.get(&property) == Some(&direct.count))
            .then_some(property)
        })
        .collect()
}

struct UnresolvedBindingIdCollector<'a> {
    name: &'a str,
    unresolved_mark: Mark,
    ids: HashSet<Id>,
}

impl<'a> UnresolvedBindingIdCollector<'a> {
    fn new(name: &'a str, unresolved_mark: Mark) -> Self {
        Self {
            name,
            unresolved_mark,
            ids: HashSet::new(),
        }
    }
}

impl Visit for UnresolvedBindingIdCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if is_unresolved_ident(ident, self.name, self.unresolved_mark) {
            self.ids.insert((ident.sym.clone(), ident.ctxt));
        }
    }
}

/// Replace only reads whose CommonJS object identity has already been proven.
/// A direct `exports.method()` call additionally requires a
/// receiver-insensitive function because the original supplies the CommonJS
/// object as `this`. Function declarations are skipped because hoisting allows
/// a later declaration body to run before an earlier-looking export
/// assignment.
fn recover_stable_commonjs_reads(
    body: &mut [ModuleItem],
    unresolved_mark: Mark,
    evidence: &CommonJsReadRecoveryEvidence,
) {
    if evidence.stable_default_assignment_span.is_none()
        && evidence.stable_named_properties.is_empty()
    {
        return;
    }

    let uses = BindingUseIndex::collect_module_items(body);
    let mut default_binding = None;
    let mut stable_named_bindings = HashMap::new();
    let mut named_bindings = HashMap::new();

    for item in body {
        item.visit_mut_with(&mut CommonJsReadRewriter {
            unresolved_mark,
            default_binding: default_binding.as_ref(),
            named_bindings: &named_bindings,
        });

        collect_stable_named_bindings(item, &uses, &mut stable_named_bindings);

        if evidence
            .stable_default_assignment_span
            .is_some_and(|span| default_export_ident_at_span(item, span).is_some())
        {
            default_binding = evidence
                .stable_default_assignment_span
                .and_then(|span| default_export_ident_at_span(item, span))
                .cloned();
        }

        collect_available_named_export_bindings(
            item,
            &evidence.stable_named_properties,
            &stable_named_bindings,
            &mut named_bindings,
        );
    }
}

fn collect_stable_named_bindings(
    item: &ModuleItem,
    uses: &BindingUseIndex,
    bindings: &mut HashMap<Id, bool>,
) {
    let declaration = match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(declaration)))
        | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(declaration),
            ..
        })) => Some(declaration.as_ref()),
        _ => None,
    };
    let Some(declaration) = declaration else {
        return;
    };
    for declarator in &declaration.decls {
        let Pat::Ident(binding) = &declarator.name else {
            continue;
        };
        let Some(init) = declarator.init.as_deref().map(strip_parens) else {
            continue;
        };
        let id = (binding.id.sym.clone(), binding.id.ctxt);
        if !uses.has_direct_write(&id) {
            bindings.insert(id, is_receiver_insensitive_function_value(init));
        }
    }
}

fn is_receiver_insensitive_function_value(expression: &Expr) -> bool {
    match expression {
        Expr::Arrow(_) => true,
        Expr::Fn(function) => !function_observes_receiver(&function.function),
        _ => false,
    }
}

fn function_observes_receiver(function: &Function) -> bool {
    let mut analyzer = ReceiverSensitivityAnalyzer::default();
    function.params.visit_with(&mut analyzer);
    function.body.visit_with(&mut analyzer);
    analyzer.sensitive
}

#[derive(Default)]
struct ReceiverSensitivityAnalyzer {
    sensitive: bool,
}

impl Visit for ReceiverSensitivityAnalyzer {
    fn visit_this_expr(&mut self, _: &ThisExpr) {
        self.sensitive = true;
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(source) = direct_eval_call_source(call) {
            let this_name: Atom = "this".into();
            self.sensitive |= match source {
                EvalCallSource::NoSource => false,
                EvalCallSource::Known(source) => js_source_mentions_binding(&source, &this_name),
                EvalCallSource::Unknown => true,
            };
            for argument in &call.args {
                argument.expr.visit_with(self);
            }
            return;
        }
        call.visit_children_with(self);
    }

    // Nested ordinary functions establish their own receiver. Arrows retain
    // the default traversal because they capture this function's receiver.
    fn visit_function(&mut self, _: &Function) {}
}

fn collect_available_named_export_bindings(
    item: &ModuleItem,
    candidates: &HashSet<Atom>,
    stable_bindings: &HashMap<Id, bool>,
    available: &mut HashMap<Atom, StableNamedBinding>,
) {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            decl: Decl::Var(declaration),
            ..
        })) => {
            for declarator in &declaration.decls {
                let Pat::Ident(binding) = &declarator.name else {
                    continue;
                };
                let property = binding.id.sym.clone();
                let id = (binding.id.sym.clone(), binding.id.ctxt);
                if let Some(receiver_insensitive) = stable_bindings.get(&id) {
                    if candidates.contains(&property) {
                        available.insert(
                            property,
                            StableNamedBinding {
                                ident: binding.id.clone(),
                                receiver_insensitive: *receiver_insensitive,
                            },
                        );
                    }
                }
            }
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if export.src.is_none() => {
            for specifier in &export.specifiers {
                let ExportSpecifier::Named(specifier) = specifier else {
                    continue;
                };
                let ModuleExportName::Ident(local) = &specifier.orig else {
                    continue;
                };
                let property = match &specifier.exported {
                    Some(ModuleExportName::Ident(exported)) => exported.sym.clone(),
                    Some(ModuleExportName::Str(_)) => continue,
                    None => local.sym.clone(),
                };
                let id = (local.sym.clone(), local.ctxt);
                if let Some(receiver_insensitive) = stable_bindings.get(&id) {
                    if candidates.contains(&property) {
                        available.insert(
                            property,
                            StableNamedBinding {
                                ident: local.clone(),
                                receiver_insensitive: *receiver_insensitive,
                            },
                        );
                    }
                }
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
struct StableNamedBinding {
    ident: Ident,
    receiver_insensitive: bool,
}

fn default_export_ident_at_span(item: &ModuleItem, span: Span) -> Option<&Ident> {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = item else {
        return None;
    };
    if export.span != span {
        return None;
    }
    let Expr::Ident(binding) = strip_parens(&export.expr) else {
        return None;
    };
    Some(binding)
}

struct CommonJsReadRewriter<'a> {
    unresolved_mark: Mark,
    default_binding: Option<&'a Ident>,
    named_bindings: &'a HashMap<Atom, StableNamedBinding>,
}

impl VisitMut for CommonJsReadRewriter<'_> {
    fn visit_mut_fn_decl(&mut self, _: &mut swc_core::ecma::ast::FnDecl) {}

    fn visit_mut_callee(&mut self, callee: &mut Callee) {
        match callee {
            Callee::Expr(expression) => self.visit_mut_call_target(expression),
            _ => callee.visit_mut_children_with(self),
        }
    }

    fn visit_mut_opt_call(&mut self, call: &mut OptCall) {
        self.visit_mut_call_target(&mut call.callee);
        call.args.visit_mut_with(self);
        call.type_args.visit_mut_with(self);
    }

    fn visit_mut_tagged_tpl(&mut self, tagged: &mut TaggedTpl) {
        self.visit_mut_call_target(&mut tagged.tag);
        tagged.tpl.visit_mut_with(self);
        tagged.type_params.visit_mut_with(self);
    }

    fn visit_mut_expr(&mut self, expression: &mut Expr) {
        if is_module_exports_expr(expression, self.unresolved_mark) {
            if let Some(binding) = self.default_binding {
                let mut binding = binding.clone();
                if let Expr::Member(member) = expression {
                    binding.span = member.span;
                }
                *expression = Expr::Ident(binding);
                return;
            }
        }

        if let Expr::Member(member) = expression {
            if let Expr::Ident(exports) = member.obj.as_ref() {
                if is_unresolved_ident(exports, "exports", self.unresolved_mark) {
                    if let Some(property) = is_ident_prop(&member.prop) {
                        if let Some(binding) = self.named_bindings.get(&property) {
                            let mut ident = binding.ident.clone();
                            ident.span = member.span;
                            *expression = Expr::Ident(ident);
                            return;
                        }
                    }
                }
            }
        }

        expression.visit_mut_children_with(self);
    }
}

impl CommonJsReadRewriter<'_> {
    fn visit_mut_call_target(&mut self, target: &mut Expr) {
        if is_module_exports_expr(strip_parens(target), self.unresolved_mark) {
            // `module.exports()` supplies `module` as the receiver. The
            // default-binding proof establishes value identity, not receiver
            // insensitivity, so direct calls stay visible and fail closed.
            return;
        }
        if let Some(property) = commonjs_named_read_property(target, self.unresolved_mark) {
            if self
                .named_bindings
                .get(&property)
                .is_some_and(|binding| binding.receiver_insensitive)
            {
                target.visit_mut_with(self);
            }
            return;
        }
        target.visit_mut_with(self);
    }
}

fn commonjs_named_read_property(expression: &Expr, unresolved_mark: Mark) -> Option<Atom> {
    let Expr::Member(member) = strip_parens(expression) else {
        return None;
    };
    let Expr::Ident(exports) = member.obj.as_ref() else {
        return None;
    };
    if !is_unresolved_ident(exports, "exports", unresolved_mark) {
        return None;
    }
    is_ident_prop(&member.prop)
}

/// Merge adjacent `var/let/const X = expr;` + `export { X };` into `export var/let/const X = expr;`.
/// Preserves the original declaration kind.
/// This pattern arises when `split_compound_exports` splits `var X = exports.X = expr`.
fn merge_decl_and_named_export(body: &mut Vec<ModuleItem>) {
    let mut i = 0;
    while i + 1 < body.len() {
        // Check if body[i] is a single-binding var decl and body[i+1] is `export { name }`
        let merged = 'merge: {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = &body[i] else {
                break 'merge false;
            };
            if var_decl.decls.len() != 1 {
                break 'merge false;
            }
            let Pat::Ident(binding) = &var_decl.decls[0].name else {
                break 'merge false;
            };
            if var_decl.decls[0].init.is_none() {
                break 'merge false;
            }
            let ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(named)) = &body[i + 1] else {
                break 'merge false;
            };
            if named.src.is_some() || named.specifiers.len() != 1 {
                break 'merge false;
            }
            let ExportSpecifier::Named(spec) = &named.specifiers[0] else {
                break 'merge false;
            };
            if spec.exported.is_some() {
                break 'merge false;
            }
            let ModuleExportName::Ident(export_id) = &spec.orig else {
                break 'merge false;
            };
            if export_id.sym != binding.id.sym || export_id.ctxt != binding.id.ctxt {
                break 'merge false;
            }
            true
        };

        if merged {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = body.remove(i) else {
                unreachable!();
            };
            let orig_span = var_decl.span;
            let kind = var_decl.kind;
            let decl = var_decl.decls.into_iter().next().unwrap();
            body[i] = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: orig_span,
                decl: Decl::Var(Box::new(VarDecl {
                    span: orig_span,
                    ctxt: Default::default(),
                    kind,
                    declare: false,
                    decls: vec![decl],
                })),
            }));
        }
        i += 1;
    }
}

fn inline_adjacent_default_export_aliases(body: &mut Vec<ModuleItem>) {
    let mut index = 0;
    while index + 1 < body.len() {
        let Some((alias, init)) = default_export_alias_decl(&body[index]) else {
            index += 1;
            continue;
        };
        let Some(export_ident) = default_export_ident(&body[index + 1]) else {
            index += 1;
            continue;
        };
        if !same_ident(&alias, export_ident) {
            index += 1;
            continue;
        }

        let alias_key = (alias.sym.clone(), alias.ctxt);
        if count_binding_refs_excluding_item(body, &alias_key, index) != 1 {
            index += 1;
            continue;
        }

        let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = &mut body[index + 1]
        else {
            index += 1;
            continue;
        };
        export.expr = init;
        body.remove(index);
    }
}

fn default_export_alias_decl(item: &ModuleItem) -> Option<(Ident, Box<Expr>)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let Some(init) = &decl.init else {
        return None;
    };
    if !matches!(init.as_ref(), Expr::Ident(_)) {
        return None;
    }
    Some((binding.id.clone(), init.clone()))
}

fn default_export_ident(item: &ModuleItem) -> Option<&Ident> {
    let ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export)) = item else {
        return None;
    };
    let Expr::Ident(id) = export.expr.as_ref() else {
        return None;
    };
    Some(id)
}

fn count_binding_refs_excluding_item(
    body: &[ModuleItem],
    key: &(Atom, SyntaxContext),
    skip_index: usize,
) -> usize {
    body.iter()
        .enumerate()
        .filter(|(index, _)| *index != skip_index)
        .map(|(_, item)| count_binding_refs(item, key))
        .sum()
}

fn get_or_insert<'a>(
    order: &mut Vec<String>,
    map: &'a mut HashMap<String, SourceEntry>,
    src: String,
) -> &'a mut SourceEntry {
    match map.entry(src.clone()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            order.push(src);
            entry.insert(SourceEntry::default())
        }
    }
}

/// Recover Babel's compiled CommonJS form of `export * from "..."`:
///
/// ```text
/// var source = require("./source.js");
/// Object.keys(source).forEach(function(key) {
///   key !== "default" && key !== "__esModule" &&
///     (key in exports && exports[key] === source[key] ||
///       (exports[key] = source[key]));
/// });
/// ```
///
/// The exact copy guard matters: a generic `Object.keys(require)` loop can
/// perform arbitrary work. The require binding must also have no uses outside
/// this adjacent pair before it is replaced with a native live re-export.
fn rewrite_commonjs_export_star_loops(module: &mut Module, unresolved_mark: Mark) {
    let binding_uses = BindingUseIndex::collect(module);
    let mut body = std::mem::take(&mut module.body).into_iter().peekable();
    let mut rewritten = Vec::with_capacity(body.size_hint().0);

    while let Some(item) = body.next() {
        let Some(next) = body.peek() else {
            rewritten.push(item);
            break;
        };
        let Some((binding, source, span)) = extract_single_require_binding(&item, unresolved_mark)
        else {
            rewritten.push(item);
            continue;
        };
        if binding_uses.use_count(&binding_id(&binding)) != 3
            || !is_commonjs_export_star_loop(next, &binding, unresolved_mark)
        {
            rewritten.push(item);
            continue;
        }

        body.next();
        rewritten.push(ModuleItem::ModuleDecl(ModuleDecl::ExportAll(ExportAll {
            span,
            src: Box::new(make_str(&source)),
            type_only: false,
            with: None,
        })));
    }

    module.body = rewritten;
}

fn extract_single_require_binding(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(Ident, String, Span)> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    let Expr::Call(call) = strip_parens(declarator.init.as_deref()?) else {
        return None;
    };
    Some((
        binding.id.clone(),
        is_require_call(call, unresolved_mark)?,
        var.span,
    ))
}

fn is_commonjs_export_star_loop(item: &ModuleItem, source: &Ident, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return false;
    };
    let Expr::Call(for_each_call) = strip_parens(expr_stmt.expr.as_ref()) else {
        return false;
    };
    if for_each_call.args.len() != 1 || for_each_call.args[0].spread.is_some() {
        return false;
    }
    let Callee::Expr(for_each_callee) = &for_each_call.callee else {
        return false;
    };
    let Expr::Member(for_each_member) = strip_parens(for_each_callee.as_ref()) else {
        return false;
    };
    if !matches!(&for_each_member.prop, MemberProp::Ident(prop) if prop.sym == "forEach") {
        return false;
    }
    let Expr::Call(keys_call) = strip_parens(for_each_member.obj.as_ref()) else {
        return false;
    };
    if keys_call.args.len() != 1
        || keys_call.args[0].spread.is_some()
        || !matches!(keys_call.args[0].expr.as_ref(), Expr::Ident(id) if same_ident(id, source))
    {
        return false;
    }
    let Callee::Expr(keys_callee) = &keys_call.callee else {
        return false;
    };
    if !is_unresolved_member_expr(keys_callee.as_ref(), "Object", "keys", unresolved_mark) {
        return false;
    }

    let Some((key, copy_expr)) = export_star_callback(for_each_call.args[0].expr.as_ref()) else {
        return false;
    };
    let mut operands = Vec::new();
    flatten_logical_and(copy_expr, &mut operands);
    operands.len() == 3
        && is_key_not_string(operands[0], &key, "default")
        && is_key_not_string(operands[1], &key, "__esModule")
        && is_guarded_commonjs_export_copy(operands[2], source, &key, unresolved_mark)
}

fn export_star_callback(expr: &Expr) -> Option<(Ident, &Expr)> {
    match strip_parens(expr) {
        Expr::Fn(function) => {
            if function.ident.is_some()
                || function.function.params.len() != 1
                || function.function.is_async
                || function.function.is_generator
            {
                return None;
            }
            let Pat::Ident(key) = &function.function.params[0].pat else {
                return None;
            };
            Some((
                key.id.clone(),
                single_expr_stmt(function.function.body.as_ref()?)?,
            ))
        }
        Expr::Arrow(arrow) => {
            if arrow.params.len() != 1 || arrow.is_async || arrow.is_generator {
                return None;
            }
            let Pat::Ident(key) = &arrow.params[0] else {
                return None;
            };
            let expr = match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(block) => single_expr_stmt(block)?,
                ArrowFunctionBody::Expr(expr) => expr.as_ref(),
            };
            Some((key.id.clone(), expr))
        }
        _ => None,
    }
}

fn single_expr_stmt(block: &FunctionBody) -> Option<&Expr> {
    if block.stmts.len() != 1 {
        return None;
    }
    let Stmt::Expr(expr_stmt) = &block.stmts[0] else {
        return None;
    };
    Some(expr_stmt.expr.as_ref())
}

fn flatten_logical_and<'a>(expr: &'a Expr, operands: &mut Vec<&'a Expr>) {
    let expr = strip_parens(expr);
    if let Expr::Bin(binary) = expr {
        if binary.op == BinaryOp::LogicalAnd {
            flatten_logical_and(binary.left.as_ref(), operands);
            flatten_logical_and(binary.right.as_ref(), operands);
            return;
        }
    }
    operands.push(expr);
}

fn is_key_not_string(expr: &Expr, key: &Ident, expected: &str) -> bool {
    let Expr::Bin(binary) = strip_parens(expr) else {
        return false;
    };
    if binary.op != BinaryOp::NotEqEq {
        return false;
    }
    (matches!(strip_parens(binary.left.as_ref()), Expr::Ident(id) if same_ident(id, key))
        && matches!(strip_parens(binary.right.as_ref()), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(expected)))
        || (matches!(strip_parens(binary.right.as_ref()), Expr::Ident(id) if same_ident(id, key))
            && matches!(strip_parens(binary.left.as_ref()), Expr::Lit(Lit::Str(value)) if value.value.as_str() == Some(expected)))
}

fn is_guarded_commonjs_export_copy(
    expr: &Expr,
    source: &Ident,
    key: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Bin(or) = strip_parens(expr) else {
        return false;
    };
    if or.op != BinaryOp::LogicalOr {
        return false;
    }
    let Expr::Bin(existing_and_equal) = strip_parens(or.left.as_ref()) else {
        return false;
    };
    if existing_and_equal.op != BinaryOp::LogicalAnd
        || !is_key_in_exports(existing_and_equal.left.as_ref(), key, unresolved_mark)
        || !is_exports_key_equal_source_key(
            existing_and_equal.right.as_ref(),
            source,
            key,
            unresolved_mark,
        )
    {
        return false;
    }

    let Expr::Assign(assign) = strip_parens(or.right.as_ref()) else {
        return false;
    };
    assign.op == AssignOp::Assign
        && matches!(&assign.left,
            AssignTarget::Simple(SimpleAssignTarget::Member(member))
                if is_computed_key_member(member, "exports", None, key, unresolved_mark))
        && matches!(strip_parens(assign.right.as_ref()), Expr::Member(member)
            if is_computed_key_member(member, "", Some(source), key, unresolved_mark))
}

fn is_key_in_exports(expr: &Expr, key: &Ident, unresolved_mark: Mark) -> bool {
    let Expr::Bin(binary) = strip_parens(expr) else {
        return false;
    };
    binary.op == BinaryOp::In
        && matches!(strip_parens(binary.left.as_ref()), Expr::Ident(id) if same_ident(id, key))
        && matches!(strip_parens(binary.right.as_ref()), Expr::Ident(id)
            if is_unresolved_ident(id, "exports", unresolved_mark))
}

fn is_exports_key_equal_source_key(
    expr: &Expr,
    source: &Ident,
    key: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Bin(binary) = strip_parens(expr) else {
        return false;
    };
    binary.op == BinaryOp::EqEqEq
        && matches!(strip_parens(binary.left.as_ref()), Expr::Member(member)
            if is_computed_key_member(member, "exports", None, key, unresolved_mark))
        && matches!(strip_parens(binary.right.as_ref()), Expr::Member(member)
            if is_computed_key_member(member, "", Some(source), key, unresolved_mark))
}

fn is_computed_key_member(
    member: &MemberExpr,
    unresolved_object: &str,
    bound_object: Option<&Ident>,
    key: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let object_matches = match (member.obj.as_ref(), bound_object) {
        (Expr::Ident(object), Some(bound)) => same_ident(object, bound),
        (Expr::Ident(object), None) => {
            is_unresolved_ident(object, unresolved_object, unresolved_mark)
        }
        _ => false,
    };
    object_matches
        && matches!(&member.prop, MemberProp::Computed(computed)
            if matches!(strip_parens(computed.expr.as_ref()), Expr::Ident(id) if same_ident(id, key)))
}

fn rewrite_webpack_export_getters(module: &mut Module, unresolved_mark: Mark) {
    expose_unused_iife_webpack_export_getters(module, unresolved_mark);

    let mut converted_getter_map = false;
    let mut new_body = Vec::with_capacity(module.body.len());
    // Webpack5 getter maps appear at the top of the module, before the
    // declarations they reference.  Deferring all converted exports to the
    // end of the body (a) avoids TDZ violations for `export default ident`
    // and (b) places `exports.X = X` adjacent to its `const X = ...`
    // declaration so merge_decl_and_named_export can fold them into
    // `export const X = ...`.
    let mut deferred_named: Vec<ModuleItem> = Vec::new();
    let mut deferred_default: Vec<ModuleItem> = Vec::new();

    for item in std::mem::take(&mut module.body) {
        let item_span = module_item_span(&item);
        if let Some(exports) = extract_direct_webpack_export_getters(&item, unresolved_mark) {
            for (name, expr) in exports {
                if name.as_ref() == "default" {
                    deferred_default.push(make_deferred_webpack_default_export(
                        item_span,
                        expr,
                        unresolved_mark,
                    ));
                } else {
                    deferred_named.push(make_exports_assign_expr_item(
                        item_span,
                        (name, expr),
                        unresolved_mark,
                    ));
                }
            }
            continue;
        }

        if let Some(exports) = extract_webpack_export_getter_iife(&item, unresolved_mark) {
            converted_getter_map = true;
            for (name, expr) in exports {
                if name.as_ref() == "default" {
                    // The getter map commonly precedes the declaration it
                    // references. Keep default live and defer it past the
                    // declarations just like the direct `require.d` form.
                    deferred_default.push(make_deferred_webpack_default_export(
                        item_span,
                        expr,
                        unresolved_mark,
                    ));
                } else {
                    // Keep named assignments in place so the ordinary export
                    // classifier can merge them with nearby declarations.
                    new_body.push(make_exports_assign_expr_item(
                        item_span,
                        (name, expr),
                        unresolved_mark,
                    ));
                }
            }
            continue;
        }

        if converted_getter_map && is_exports_default_compat_postamble(&item, unresolved_mark) {
            continue;
        }

        new_body.push(item);
    }

    // Named exports first (adjacent to their declarations for merging),
    // then default exports last (after all declarations to avoid TDZ).
    new_body.extend(deferred_named);
    new_body.extend(deferred_default);
    module.body = new_body;
}

/// Rewrite ncc's CommonJS default-object adapter when the complete generated
/// export surface is exactly one live `default` getter.
///
/// In that exact shape, `Object.assign(exports.default, exports)` copies only
/// the enumerable `default` getter back onto the default value itself. Keep
/// that self mirror and the observable `__esModule` definition on the
/// recovered binding, while native ESM replaces the final `module.exports`
/// reassignment.
///
/// This is deliberately not inferred from the eventual printed export. The
/// proof requires one top-level generated getter, one final exact postamble,
/// no pre-existing ESM declarations, and no other unresolved `exports` or
/// `module` use anywhere in the module. Named properties, aliases, escapes,
/// hidden mutations, and direct eval therefore fail closed.
fn rewrite_recovered_default_only_default_compat_block(module: &mut Module, unresolved_mark: Mark) {
    let compat_indices: Vec<usize> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            extract_strict_default_compat_parts(item, unresolved_mark)
                .is_some()
                .then_some(index)
        })
        .collect();
    let [compat_index] = compat_indices.as_slice() else {
        return;
    };
    if *compat_index + 1 != module.body.len() {
        return;
    }

    // Authored ESM mixed with a CommonJS facade needs a different provenance
    // model. This recovery is only for a fully generated CommonJS surface.
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return;
    }

    let default_getters: Vec<(usize, Ident)> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            extract_exports_default_ident_getter(item, unresolved_mark)
                .map(|binding| (index, binding))
        })
        .collect();
    let [(default_getter_index, default_binding)] = default_getters.as_slice() else {
        return;
    };
    if default_getter_index >= compat_index {
        return;
    }

    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        return;
    }

    for (index, item) in module.body.iter().enumerate() {
        if index == *compat_index || index == *default_getter_index {
            continue;
        }

        let mut exports_ids = UnresolvedBindingIdCollector::new("exports", unresolved_mark);
        item.visit_with(&mut exports_ids);
        if !exports_ids.ids.is_empty() {
            return;
        }

        let mut module_ids = UnresolvedBindingIdCollector::new("module", unresolved_mark);
        item.visit_with(&mut module_ids);
        if !module_ids.ids.is_empty() {
            return;
        }
    }

    let Some(rewritten) = make_default_binding_compat_block(
        &module.body[*compat_index],
        default_binding,
        unresolved_mark,
    ) else {
        return;
    };
    module.body[*compat_index] = rewritten;
}

/// Remove ncc's CommonJS default-compatibility postamble when the complete
/// CommonJS surface proves that `exports.default` can never exist.
///
/// Absence of an emitted `export default` is not evidence by itself. The
/// proof below starts from the same binding-use index as CommonJS self-read
/// recovery and accepts only static, non-default `exports.name` accesses.
/// Computed access, whole-object escape/aliasing, an unconverted export-getter
/// helper, any meaningful `module` use, and direct eval all fail closed. Since
/// the index traverses every function body, a later hoisted function
/// declaration cannot hide a default write from the proof.
///
/// Static member access is only property-creation-free for ordinary names:
/// `exports.__proto__ = obj` makes `exports.default` resolvable through the
/// new prototype, and `exports.__defineGetter__(...)` can install a `default`
/// accessor while surfacing as a plain member read. Those prototype-mutating
/// names fail closed too.
fn remove_dead_named_only_default_compat_blocks(module: &mut Module, unresolved_mark: Mark) {
    let compat_indices: HashSet<usize> = module
        .body
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            is_exports_default_compat_postamble(item, unresolved_mark).then_some(index)
        })
        .collect();
    if compat_indices.is_empty() {
        return;
    }

    // A recovered default getter is already native ESM by this phase and no
    // longer appears as an `exports.default` use. Keep case 2 intact: its
    // CommonJS default-object compatibility needs a separate value model.
    if has_esm_default_export(module) {
        return;
    }

    let mut direct_eval = DirectEvalPresence::default();
    module.visit_with(&mut direct_eval);
    if direct_eval.found {
        return;
    }

    // SimplifySequence commonly synthesizes this `if` from an `&&` / comma
    // expression, so its span may be dummy. Exclude the exact top-level items
    // by identity instead of using source ranges as semantic evidence.
    let uses = BindingUseIndex::collect_module_items_excluding(&module.body, &compat_indices);
    let mut exports_ids = UnresolvedBindingIdCollector::new("exports", unresolved_mark);
    for (index, item) in module.body.iter().enumerate() {
        if !compat_indices.contains(&index) {
            item.visit_with(&mut exports_ids);
        }
    }
    let exports_are_named_only = exports_ids.ids.iter().all(|binding| {
        uses.use_sites(binding).iter().all(|site| {
            matches!(
                &site.kind,
                UseKind::StaticMemberRead(property) | UseKind::StaticMemberWrite(property)
                    if property.as_ref() != "default"
                        && !is_prototype_mutating_member_name(property.as_ref())
            )
        })
    });
    if !exports_are_named_only {
        return;
    }

    let mut module_ids = UnresolvedBindingIdCollector::new("module", unresolved_mark);
    for (index, item) in module.body.iter().enumerate() {
        if !compat_indices.contains(&index) {
            item.visit_with(&mut module_ids);
        }
    }
    let module_is_only_in_compat = module_ids
        .ids
        .iter()
        .all(|binding| uses.use_sites(binding).is_empty());
    if !module_is_only_in_compat {
        return;
    }

    let mut index = 0usize;
    module.body.retain(|_| {
        let keep = !compat_indices.contains(&index);
        index += 1;
        keep
    });
}

fn has_esm_default_export(module: &Module) -> bool {
    module.body.iter().any(|item| match item {
        ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_),
        ) => true,
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) => {
            export.specifiers.iter().any(|specifier| match specifier {
                ExportSpecifier::Named(named) => {
                    named
                        .exported
                        .as_ref()
                        .unwrap_or(&named.orig)
                        .atom()
                        .as_ref()
                        == "default"
                }
                ExportSpecifier::Default(default) => default.exported.sym.as_ref() == "default",
                ExportSpecifier::Namespace(namespace) => {
                    namespace.name.atom().as_ref() == "default"
                }
            })
        }
        _ => false,
    })
}

fn make_deferred_webpack_default_export(
    span: Span,
    expr: Box<Expr>,
    unresolved_mark: Mark,
) -> ModuleItem {
    if let Expr::Ident(ident) = *expr {
        // Webpack getter `() => ident` is a live accessor. Emit a live export
        // specifier directly instead of routing through CJS classification,
        // which would snapshot the binding.
        ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
            span,
            specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                span: DUMMY_SP,
                orig: ModuleExportName::Ident(ident),
                exported: Some(ModuleExportName::Ident(
                    IdentName::new("default".into(), DUMMY_SP).into(),
                )),
                is_type_only: false,
            })],
            src: None,
            type_only: false,
            with: None,
        }))
    } else {
        make_exports_assign_expr_item(span, ("default".into(), expr), unresolved_mark)
    }
}

fn expose_unused_iife_webpack_export_getters(module: &mut Module, unresolved_mark: Mark) {
    let candidate_index = module
        .body
        .iter()
        .take_while(|item| is_use_strict_module_item(item))
        .count();
    if candidate_index + 1 != module.body.len() {
        return;
    }
    let Some(expanded) = extract_unused_iife_webpack_export_getter_body(
        &module.body[candidate_index],
        unresolved_mark,
    ) else {
        return;
    };
    module.body.truncate(candidate_index);
    module.body.extend(expanded);
}

fn is_use_strict_module_item(item: &ModuleItem) -> bool {
    matches!(item, ModuleItem::Stmt(Stmt::Expr(statement))
        if matches!(strip_parens(&statement.expr), Expr::Lit(Lit::Str(value))
            if value.value.as_str() == Some("use strict")))
}

fn extract_unused_iife_webpack_export_getter_body(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Arrow(arrow) = strip_parens(callee_expr.as_ref()) else {
        return None;
    };
    let ArrowFunctionBody::FunctionBody(block) = arrow.body.as_ref() else {
        return None;
    };
    if block_has_top_level_return(block) || block_contains_arguments_ident(block) {
        return None;
    }
    if !block_contains_direct_webpack_export_getter(block, unresolved_mark) {
        return None;
    }
    if arrow_params_used_in_block(arrow, block) {
        return None;
    }

    let outer_span = expr_stmt.span;
    let mut items = Vec::with_capacity(call.args.len() + block.stmts.len());
    items.extend(call.args.iter().map(|arg| {
        ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: outer_span,
            expr: arg.expr.clone(),
        }))
    }));
    items.extend(block.stmts.iter().cloned().map(ModuleItem::Stmt));
    Some(items)
}

fn block_contains_direct_webpack_export_getter(
    block: &FunctionBody,
    unresolved_mark: Mark,
) -> bool {
    block.stmts.iter().any(|stmt| {
        extract_direct_webpack_export_getters(&ModuleItem::Stmt(stmt.clone()), unresolved_mark)
            .is_some()
    })
}

fn block_has_top_level_return(block: &FunctionBody) -> bool {
    block
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, Stmt::Return(_)))
}

fn arrow_params_used_in_block(arrow: &ArrowExpr, block: &FunctionBody) -> bool {
    let params: Vec<Ident> = arrow
        .params
        .iter()
        .filter_map(|param| match param {
            Pat::Ident(binding) => Some(binding.id.clone()),
            _ => None,
        })
        .collect();
    if params.len() != arrow.params.len() {
        return true;
    }
    if params.is_empty() {
        return false;
    }

    let mut finder = IdentUseFinder {
        targets: &params,
        found: false,
    };
    block.visit_with(&mut finder);
    finder.found
}

struct IdentUseFinder<'a> {
    targets: &'a [Ident],
    found: bool,
}

impl Visit for IdentUseFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.targets.iter().any(|target| same_ident(ident, target)) {
            self.found = true;
        }
    }
}

fn block_contains_arguments_ident(block: &FunctionBody) -> bool {
    let mut finder = ArgumentsIdentFinder { found: false };
    block.visit_with(&mut finder);
    finder.found
}

struct ArgumentsIdentFinder {
    found: bool,
}

impl Visit for ArgumentsIdentFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == "arguments" {
            self.found = true;
        }
    }
}

fn extract_direct_webpack_export_getters(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<(Atom, Box<Expr>)>> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return None;
    };
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    if !is_unresolved_member_expr(callee_expr.as_ref(), "require", "d", unresolved_mark) {
        return None;
    }
    if call.args.is_empty() {
        return None;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "exports", unresolved_mark))
    {
        return None;
    }

    if call.args.len() == 2 {
        let Expr::Object(getter_map) = call.args[1].expr.as_ref() else {
            return None;
        };
        let exports = extract_export_getter_map(getter_map)?;
        if exports.is_empty() {
            return None;
        }
        return Some(exports);
    }

    if call.args.len() == 3 {
        let Expr::Lit(Lit::Str(name)) = call.args[1].expr.as_ref() else {
            return None;
        };
        let export_name = name.value.as_str()?;
        if !is_valid_js_ident(export_name) || is_prototype_mutating_member_name(export_name) {
            return None;
        }
        let expr = extract_getter_expr_return_expr(call.args[2].expr.as_ref())?;
        return Some(vec![(export_name.into(), expr)]);
    }

    None
}

fn extract_webpack_export_getter_iife(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<(Atom, Box<Expr>)>> {
    let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
        return None;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if call.args.len() != 2 {
        return None;
    }

    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    let Expr::Arrow(arrow) = strip_parens(callee_expr.as_ref()) else {
        return None;
    };
    let (target_param, map_param) = extract_two_ident_params(arrow)?;
    if !is_webpack_export_getter_loop(arrow, &target_param, &map_param) {
        return None;
    }

    if !matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "exports", unresolved_mark))
    {
        return None;
    }
    let Expr::Object(getter_map) = call.args[1].expr.as_ref() else {
        return None;
    };

    let exports = extract_export_getter_map(getter_map)?;
    (!exports.is_empty()).then_some(exports)
}

fn extract_two_ident_params(arrow: &ArrowExpr) -> Option<(Ident, Ident)> {
    if arrow.params.len() != 2 {
        return None;
    }
    let Pat::Ident(target) = &arrow.params[0] else {
        return None;
    };
    let Pat::Ident(map) = &arrow.params[1] else {
        return None;
    };
    Some((target.id.clone(), map.id.clone()))
}

fn is_webpack_export_getter_loop(
    arrow: &ArrowExpr,
    target_param: &Ident,
    map_param: &Ident,
) -> bool {
    let ArrowFunctionBody::FunctionBody(block) = arrow.body.as_ref() else {
        return false;
    };
    if block.stmts.len() != 1 {
        return false;
    }
    let Stmt::ForIn(ForInStmt {
        left, right, body, ..
    }) = &block.stmts[0]
    else {
        return false;
    };
    if !matches!(right.as_ref(), Expr::Ident(id) if same_ident(id, map_param)) {
        return false;
    }
    let Some(loop_ident) = extract_for_in_ident(left) else {
        return false;
    };

    let Stmt::Block(body_block) = body.as_ref() else {
        return false;
    };
    if body_block.stmts.len() != 1 {
        return false;
    }
    let Stmt::Expr(ExprStmt { expr, .. }) = &body_block.stmts[0] else {
        return false;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return false;
    };
    is_object_define_property_call(call, target_param, &loop_ident, map_param)
}

fn extract_for_in_ident(left: &ForHead) -> Option<Ident> {
    match left {
        ForHead::VarDecl(var) => {
            if var.decls.len() != 1 {
                return None;
            }
            let decl = &var.decls[0];
            if decl.init.is_some() {
                return None;
            }
            let Pat::Ident(binding) = &decl.name else {
                return None;
            };
            Some(binding.id.clone())
        }
        ForHead::Pat(pat) => {
            let Pat::Ident(binding) = pat.as_ref() else {
                return None;
            };
            Some(binding.id.clone())
        }
        _ => None,
    }
}

fn is_object_define_property_call(
    call: &CallExpr,
    target_param: &Ident,
    loop_ident: &Ident,
    map_param: &Ident,
) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    if !is_member_expr(callee_expr.as_ref(), "Object", "defineProperty") || call.args.len() != 3 {
        return false;
    }
    if !matches!(call.args[0].expr.as_ref(), Expr::Ident(id) if same_ident(id, target_param)) {
        return false;
    }
    if !matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if same_ident(id, loop_ident)) {
        return false;
    }
    is_export_getter_descriptor(call.args[2].expr.as_ref(), map_param, loop_ident)
}

fn is_export_getter_descriptor(expr: &Expr, map_param: &Ident, loop_ident: &Ident) -> bool {
    let Expr::Object(object) = expr else {
        return false;
    };
    let mut has_enumerable_true = false;
    let mut has_getter_lookup = false;

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return false;
        };
        match prop_name_as_atom(&entry.key).as_deref() {
            Some("enumerable") => {
                has_enumerable_true =
                    matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value);
            }
            Some("get") => {
                has_getter_lookup = is_map_lookup(entry.value.as_ref(), map_param, loop_ident);
            }
            _ => return false,
        }
    }

    has_enumerable_true && has_getter_lookup
}

fn extract_export_getter_map(
    object: &swc_core::ecma::ast::ObjectLit,
) -> Option<Vec<(Atom, Box<Expr>)>> {
    let mut exports = Vec::with_capacity(object.props.len());
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let (name, expr) = match prop.as_ref() {
            Prop::Method(method) => {
                let name = prop_name_as_atom(&method.key)?;
                if !method.function.params.is_empty()
                    || method.function.is_async
                    || method.function.is_generator
                {
                    return None;
                }
                let expr = extract_single_return_expr(method.function.body.as_ref()?)?;
                (name, expr)
            }
            Prop::KeyValue(entry) => {
                let name = prop_name_as_atom(&entry.key)?;
                let expr = extract_getter_expr_return_expr(entry.value.as_ref())?;
                (name, expr)
            }
            _ => return None,
        };
        // Converting such a name would synthesize `exports.__proto__ = expr`,
        // a prototype write instead of the original own accessor. Keep the
        // whole helper call as a residual.
        if is_prototype_mutating_member_name(name.as_ref()) {
            return None;
        }
        exports.push((name, expr));
    }
    Some(exports)
}

fn extract_getter_expr_return_expr(expr: &Expr) -> Option<Box<Expr>> {
    match expr {
        Expr::Fn(fn_expr) => {
            if fn_expr.ident.is_some()
                || !fn_expr.function.params.is_empty()
                || fn_expr.function.is_async
                || fn_expr.function.is_generator
            {
                return None;
            }
            extract_single_return_expr(fn_expr.function.body.as_ref()?)
        }
        Expr::Arrow(arrow) => {
            if !arrow.params.is_empty() || arrow.is_async || arrow.is_generator {
                return None;
            }
            match arrow.body.as_ref() {
                ArrowFunctionBody::FunctionBody(block) => extract_single_return_expr(block),
                ArrowFunctionBody::Expr(expr) => Some(expr.clone()),
            }
        }
        _ => None,
    }
}

fn extract_single_return_expr(block: &FunctionBody) -> Option<Box<Expr>> {
    if block.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else {
        return None;
    };
    Some(arg.clone())
}

fn make_exports_assign_expr_item(
    span: Span,
    (name, expr): (Atom, Box<Expr>),
    unresolved_mark: Mark,
) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(make_unresolved_ident(
                    "exports".into(),
                    unresolved_mark,
                ))),
                prop: MemberProp::Ident(IdentName::new(name, DUMMY_SP)),
            })),
            right: expr,
        })),
    }))
}

fn is_exports_default_compat_block(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(Stmt::If(if_stmt)) = item else {
        return false;
    };
    if if_stmt.alt.is_some() {
        return false;
    }
    if !is_exports_default_compat_test(if_stmt.test.as_ref(), unresolved_mark) {
        return false;
    }
    let Stmt::Block(block) = if_stmt.cons.as_ref() else {
        return false;
    };
    if block.stmts.len() != 3 {
        return false;
    }

    is_define_esmodule_on_exports_default(&block.stmts[0], unresolved_mark)
        && is_object_assign_exports_default_exports(&block.stmts[1], unresolved_mark)
        && is_module_exports_default_reassignment(&block.stmts[2], unresolved_mark)
}

fn extract_exports_default_ident_getter(item: &ModuleItem, unresolved_mark: Mark) -> Option<Ident> {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return None;
    };
    let Expr::Call(call) = strip_parens(statement.expr.as_ref()) else {
        return None;
    };
    if !is_object_define_property_global_call(call, unresolved_mark)
        || call.args.len() != 3
        || call.args.iter().any(|arg| arg.spread.is_some())
    {
        return None;
    }
    if !matches!(strip_parens(call.args[0].expr.as_ref()), Expr::Ident(id)
        if is_unresolved_ident(id, "exports", unresolved_mark))
        || !matches!(strip_parens(call.args[1].expr.as_ref()), Expr::Lit(Lit::Str(name))
            if name.value.as_str() == Some("default"))
    {
        return None;
    }
    extract_define_property_getter_ident(call.args[2].expr.as_ref(), unresolved_mark)
}

fn make_default_binding_compat_block(
    item: &ModuleItem,
    default_binding: &Ident,
    unresolved_mark: Mark,
) -> Option<ModuleItem> {
    let (span, mut test, mut define_esmodule) =
        extract_strict_default_compat_parts(item, unresolved_mark)?;
    let mut replacer = ExportsDefaultToBinding {
        binding: default_binding.clone(),
        unresolved_mark,
    };
    test.visit_mut_with(&mut replacer);
    define_esmodule.visit_mut_with(&mut replacer);

    let self_mirror = Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span,
                obj: Box::new(Expr::Ident(default_binding.clone())),
                prop: MemberProp::Ident(IdentName::new("default".into(), span)),
            })),
            right: Box::new(Expr::Ident(default_binding.clone())),
        })),
    });

    Some(ModuleItem::Stmt(Stmt::If(IfStmt {
        span,
        test,
        cons: Box::new(Stmt::Block(BlockStmt {
            span,
            ctxt: Default::default(),
            stmts: vec![
                Stmt::Expr(ExprStmt {
                    span,
                    expr: define_esmodule,
                }),
                self_mirror,
            ],
        })),
        alt: None,
    })))
}

fn extract_strict_default_compat_parts(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<(Span, Box<Expr>, Box<Expr>)> {
    let (span, test, define_esmodule, copy_exports, assign_default) = match item {
        ModuleItem::Stmt(Stmt::If(if_stmt)) => {
            if if_stmt.alt.is_some() {
                return None;
            }
            let Stmt::Block(block) = if_stmt.cons.as_ref() else {
                return None;
            };
            let [Stmt::Expr(define_esmodule), Stmt::Expr(copy_exports), Stmt::Expr(assign_default)] =
                block.stmts.as_slice()
            else {
                return None;
            };
            (
                if_stmt.span,
                if_stmt.test.clone(),
                define_esmodule.expr.clone(),
                copy_exports.expr.as_ref(),
                assign_default.expr.as_ref(),
            )
        }
        ModuleItem::Stmt(Stmt::Expr(statement)) => {
            let Expr::Bin(postamble) = strip_parens(statement.expr.as_ref()) else {
                return None;
            };
            if postamble.op != BinaryOp::LogicalAnd {
                return None;
            }
            let Expr::Seq(sequence) = strip_parens(postamble.right.as_ref()) else {
                return None;
            };
            let [define_esmodule, copy_exports, assign_default] = sequence.exprs.as_slice() else {
                return None;
            };
            (
                statement.span,
                postamble.left.clone(),
                define_esmodule.clone(),
                copy_exports.as_ref(),
                assign_default.as_ref(),
            )
        }
        _ => return None,
    };

    if !is_rewritable_exports_default_compat_test(test.as_ref(), unresolved_mark)
        || !is_strict_define_esmodule_on_exports_default_expr(
            define_esmodule.as_ref(),
            unresolved_mark,
        )
        || !is_strict_object_assign_exports_default_exports_expr(copy_exports, unresolved_mark)
        || !is_module_exports_default_reassignment_expr(assign_default, unresolved_mark)
    {
        return None;
    }
    Some((span, test, define_esmodule))
}

fn is_rewritable_exports_default_compat_test(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::LogicalAnd
        && is_rewritable_exports_default_type_guard(bin.left.as_ref(), unresolved_mark)
        && is_exports_default_esmodule_undefined(bin.right.as_ref(), unresolved_mark)
}

fn is_rewritable_exports_default_type_guard(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::LogicalOr {
        return false;
    }
    let Expr::Bin(object_and_not_null) = strip_parens(bin.right.as_ref()) else {
        return false;
    };

    is_typeof_exports_default_eq(bin.left.as_ref(), "function", unresolved_mark)
        && object_and_not_null.op == BinaryOp::LogicalAnd
        && (is_typeof_exports_default_eq(
            object_and_not_null.left.as_ref(),
            "object",
            unresolved_mark,
        ) || is_exports_default_type_helper_eq_object(
            object_and_not_null.left.as_ref(),
            unresolved_mark,
        ))
        && is_exports_default_not_null(object_and_not_null.right.as_ref(), unresolved_mark)
}

/// The helper result does not need to be interpreted: the rewrite preserves
/// the call, its receiver, and its order, replacing only the generated live
/// getter read with the binding that getter returns. Unlike the named-only
/// dead-branch proof, this remains sound for an otherwise unknown one-argument
/// type helper.
fn is_exports_default_type_helper_eq_object(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::EqEqEq
        || !matches!(strip_parens(bin.right.as_ref()), Expr::Lit(Lit::Str(value))
            if value.value.as_str() == Some("object"))
    {
        return false;
    }
    let Expr::Call(call) = strip_parens(bin.left.as_ref()) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let mut exports_ids = UnresolvedBindingIdCollector::new("exports", unresolved_mark);
    callee.visit_with(&mut exports_ids);
    let mut module_ids = UnresolvedBindingIdCollector::new("module", unresolved_mark);
    callee.visit_with(&mut module_ids);
    call.args.len() == 1
        && call.args[0].spread.is_none()
        && is_exports_default_expr(call.args[0].expr.as_ref(), unresolved_mark)
        && exports_ids.ids.is_empty()
        && module_ids.ids.is_empty()
}

fn is_strict_define_esmodule_on_exports_default_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_unresolved_member_expr(callee.as_ref(), "Object", "defineProperty", unresolved_mark)
        && call.args.len() == 3
        && call.args.iter().all(|arg| arg.spread.is_none())
        && is_exports_default_expr(call.args[0].expr.as_ref(), unresolved_mark)
        && is_esmodule_name_arg(call.args[1].expr.as_ref())
        && is_esmodule_descriptor(call.args[2].expr.as_ref())
}

fn is_strict_object_assign_exports_default_exports_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    call.args.iter().all(|arg| arg.spread.is_none())
        && is_object_assign_exports_default_exports_expr(expr, unresolved_mark)
}

struct ExportsDefaultToBinding {
    binding: Ident,
    unresolved_mark: Mark,
}

impl VisitMut for ExportsDefaultToBinding {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if is_exports_default_expr(expr, self.unresolved_mark) {
            *expr = Expr::Ident(self.binding.clone());
            return;
        }
        expr.visit_mut_children_with(self);
    }
}

fn is_exports_default_compat_postamble(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    is_exports_default_compat_block(item, unresolved_mark)
        || is_exports_default_compat_logical_expression(item, unresolved_mark)
}

fn is_exports_default_compat_logical_expression(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let ModuleItem::Stmt(Stmt::Expr(statement)) = item else {
        return false;
    };
    let Expr::Bin(postamble) = strip_parens(statement.expr.as_ref()) else {
        return false;
    };
    postamble.op == BinaryOp::LogicalAnd
        && is_exports_default_compat_test(postamble.left.as_ref(), unresolved_mark)
        && is_exports_default_compat_sequence(postamble.right.as_ref(), unresolved_mark)
}

fn is_exports_default_compat_sequence(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Seq(sequence) = strip_parens(expr) else {
        return false;
    };
    let [define_esmodule, copy_exports, assign_default] = sequence.exprs.as_slice() else {
        return false;
    };
    is_define_esmodule_on_exports_default_expr(define_esmodule, unresolved_mark)
        && is_object_assign_exports_default_exports_expr(copy_exports, unresolved_mark)
        && is_module_exports_default_reassignment_expr(assign_default, unresolved_mark)
}

fn is_exports_default_compat_test(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::LogicalAnd
        && is_exports_default_type_guard(bin.left.as_ref(), unresolved_mark)
        && is_exports_default_esmodule_undefined(bin.right.as_ref(), unresolved_mark)
}

fn is_exports_default_type_guard(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::LogicalOr {
        return false;
    }
    let Expr::Bin(object_and_not_null) = strip_parens(bin.right.as_ref()) else {
        return false;
    };

    is_typeof_exports_default_eq(bin.left.as_ref(), "function", unresolved_mark)
        && object_and_not_null.op == BinaryOp::LogicalAnd
        && is_typeof_exports_default_eq(
            object_and_not_null.left.as_ref(),
            "object",
            unresolved_mark,
        )
        && is_exports_default_not_null(object_and_not_null.right.as_ref(), unresolved_mark)
}

fn is_typeof_exports_default_eq(expr: &Expr, expected: &str, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::EqEqEq {
        return false;
    }
    matches!(strip_parens(bin.left.as_ref()), Expr::Unary(unary)
        if unary.op == UnaryOp::TypeOf && is_exports_default_expr(unary.arg.as_ref(), unresolved_mark))
        && matches!(strip_parens(bin.right.as_ref()), Expr::Lit(Lit::Str(s))
            if s.value.as_str() == Some(expected))
}

fn is_exports_default_not_null(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::NotEqEq
        && is_exports_default_expr(bin.left.as_ref(), unresolved_mark)
        && matches!(strip_parens(bin.right.as_ref()), Expr::Lit(Lit::Null(_)))
}

fn is_exports_default_esmodule_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::EqEqEq
        && is_exports_default_esmodule_expr(bin.left.as_ref(), unresolved_mark)
        && matches!(strip_parens(bin.right.as_ref()), Expr::Ident(id) if is_undefined_ident(id, unresolved_mark))
}

fn is_exports_default_esmodule_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "__esModule")
        && is_exports_default_expr(member.obj.as_ref(), unresolved_mark)
}

fn is_define_esmodule_on_exports_default(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(statement) = stmt else {
        return false;
    };
    is_define_esmodule_on_exports_default_expr(statement.expr.as_ref(), unresolved_mark)
}

fn is_define_esmodule_on_exports_default_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    if !is_unresolved_member_expr(callee.as_ref(), "Object", "defineProperty", unresolved_mark)
        || call.args.len() != 3
    {
        return false;
    }
    if !is_exports_default_expr(call.args[0].expr.as_ref(), unresolved_mark) {
        return false;
    }
    if !matches!(call.args[1].expr.as_ref(), Expr::Lit(Lit::Str(s)) if s.value.as_str() == Some("__esModule"))
    {
        return false;
    }

    let Expr::Object(obj) = call.args[2].expr.as_ref() else {
        return false;
    };
    obj.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return false;
        };
        prop_name_as_atom(&entry.key).as_deref() == Some("value")
            && matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(b)) if b.value)
    })
}

fn is_object_assign_exports_default_exports(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(statement) = stmt else {
        return false;
    };
    is_object_assign_exports_default_exports_expr(statement.expr.as_ref(), unresolved_mark)
}

fn is_object_assign_exports_default_exports_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_unresolved_member_expr(callee.as_ref(), "Object", "assign", unresolved_mark)
        && call.args.len() == 2
        && is_exports_default_expr(call.args[0].expr.as_ref(), unresolved_mark)
        && matches!(call.args[1].expr.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "exports", unresolved_mark))
}

fn is_module_exports_default_reassignment(stmt: &Stmt, unresolved_mark: Mark) -> bool {
    let Stmt::Expr(statement) = stmt else {
        return false;
    };
    is_module_exports_default_reassignment_expr(statement.expr.as_ref(), unresolved_mark)
}

fn is_module_exports_default_reassignment_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    if assign.op != AssignOp::Assign
        || !is_exports_default_expr(assign.right.as_ref(), unresolved_mark)
    {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return false;
    };
    is_module_exports_member(member, unresolved_mark)
}

fn is_exports_default_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "exports", unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "default")
}

fn is_module_exports_member(member: &MemberExpr, unresolved_mark: Mark) -> bool {
    matches!(member.obj.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "module", unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "exports")
}

fn is_unresolved_member_expr(
    expr: &Expr,
    object: &str,
    property: &str,
    unresolved_mark: Mark,
) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, object, unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == property)
}

fn is_member_expr(expr: &Expr, object: &str, property: &str) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == object)
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == property)
}

fn is_map_lookup(expr: &Expr, map_param: &Ident, loop_ident: &Ident) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(id) if same_ident(id, map_param)) {
        return false;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return false;
    };
    matches!(computed.expr.as_ref(), Expr::Ident(id) if same_ident(id, loop_ident))
}

fn prop_name_as_atom(name: &PropName) -> Option<Atom> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.clone()),
        PropName::Str(str) => {
            let value = str.value.as_str()?;
            if is_valid_js_ident(value) {
                Some(value.into())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn build_import_decls(src: &str, entry: &SourceEntry, out: &mut Vec<ModuleItem>) {
    // Case: bare-only import (no bindings at all)
    if entry.bare
        && entry.first_default.is_none()
        && entry.named.is_empty()
        && entry.extra_defaults.is_empty()
    {
        out.push(ModuleItem::ModuleDecl(ModuleDecl::Import(
            make_import_decl(src, vec![]),
        )));
        return;
    }

    // Primary import: first_default + all named
    let mut specifiers: Vec<ImportSpecifier> = Vec::new();
    if let Some(ref def) = entry.first_default {
        specifiers.push(ImportSpecifier::Default(ImportDefaultSpecifier {
            span: DUMMY_SP,
            local: def.clone(),
        }));
    }
    for (imported, local) in &entry.named {
        if *imported == local.sym {
            specifiers.push(ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: local.clone(),
                imported: None,
                is_type_only: false,
            }));
        } else {
            specifiers.push(ImportSpecifier::Named(ImportNamedSpecifier {
                span: DUMMY_SP,
                local: local.clone(),
                imported: Some(ModuleExportName::Ident(make_ident(imported.clone()))),
                is_type_only: false,
            }));
        }
    }

    if !specifiers.is_empty() {
        out.push(ModuleItem::ModuleDecl(ModuleDecl::Import(
            make_import_decl(src, specifiers),
        )));
    }

    // Extra defaults → separate import statements
    for extra in &entry.extra_defaults {
        out.push(ModuleItem::ModuleDecl(ModuleDecl::Import(
            make_import_decl(
                src,
                vec![ImportSpecifier::Default(ImportDefaultSpecifier {
                    span: DUMMY_SP,
                    local: extra.clone(),
                })],
            ),
        )));
    }
}

fn make_import_decl(src: &str, specifiers: Vec<ImportSpecifier>) -> ImportDecl {
    ImportDecl {
        span: DUMMY_SP,
        specifiers,
        src: Box::new(make_str(src)),
        type_only: false,
        with: None,
        phase: Default::default(),
    }
}

fn build_export_items(
    span: Span,
    kind: CjsExportKind,
    used_names: &mut HashSet<Atom>,
    unresolved_reference_names: &HashSet<Atom>,
) -> Vec<ModuleItem> {
    match kind {
        CjsExportKind::EsModuleFlag => vec![],
        CjsExportKind::ModuleExportsDefault { expr } => vec![ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultExpr(ExportDefaultExpr { span, expr }),
        )],
        CjsExportKind::NamedDefault { expr } => vec![ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultExpr(ExportDefaultExpr { span, expr }),
        )],
        CjsExportKind::ReExport {
            name,
            imported,
            source,
            ..
        } => vec![ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
            NamedExport {
                span,
                specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                    span: DUMMY_SP,
                    orig: ModuleExportName::Ident(
                        IdentName::new(imported.clone(), DUMMY_SP).into(),
                    ),
                    exported: (imported != name)
                        .then(|| ModuleExportName::Ident(IdentName::new(name, DUMMY_SP).into())),
                    is_type_only: false,
                })],
                src: Some(Box::new(make_str(&source))),
                type_only: false,
                with: None,
            },
        ))],
        CjsExportKind::DefaultMirror => vec![],
        CjsExportKind::Named {
            name,
            expr,
            is_void: false,
        } => {
            if let Expr::Ident(id) = *expr {
                if id.sym == name {
                    // export { foo }
                    vec![ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
                        NamedExport {
                            span,
                            specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                                span: DUMMY_SP,
                                orig: ModuleExportName::Ident(id),
                                exported: None,
                                is_type_only: false,
                            })],
                            src: None,
                            type_only: false,
                            with: None,
                        },
                    ))]
                } else {
                    // export { id as name }
                    vec![ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
                        NamedExport {
                            span,
                            specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                                span: DUMMY_SP,
                                orig: ModuleExportName::Ident(id),
                                exported: Some(ModuleExportName::Ident(make_ident(name))),
                                is_type_only: false,
                            })],
                            src: None,
                            type_only: false,
                            with: None,
                        },
                    ))]
                }
            } else if is_reserved_binding_name(&name) || unresolved_reference_names.contains(&name)
            {
                let local = make_ident(fresh_prefixed_name(&name, used_names));
                vec![
                    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                        span,
                        ctxt: Default::default(),
                        kind: VarDeclKind::Var,
                        declare: false,
                        decls: vec![VarDeclarator {
                            span: DUMMY_SP,
                            name: Pat::Ident(BindingIdent {
                                id: local.clone(),
                                type_ann: None,
                            }),
                            init: Some(expr),
                            definite: false,
                        }],
                    })))),
                    ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(NamedExport {
                        span,
                        specifiers: vec![ExportSpecifier::Named(ExportNamedSpecifier {
                            span: DUMMY_SP,
                            orig: ModuleExportName::Ident(local),
                            exported: Some(ModuleExportName::Ident(make_ident(name))),
                            is_type_only: false,
                        })],
                        src: None,
                        type_only: false,
                        with: None,
                    })),
                ]
            } else {
                // export const name = expr
                vec![ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span,
                    decl: Decl::Var(Box::new(VarDecl {
                        span: DUMMY_SP,
                        ctxt: Default::default(),
                        kind: VarDeclKind::Const,
                        declare: false,
                        decls: vec![VarDeclarator {
                            span: DUMMY_SP,
                            name: Pat::Ident(BindingIdent {
                                id: make_ident(name),
                                type_ann: None,
                            }),
                            init: Some(expr),
                            definite: false,
                        }],
                    })),
                }))]
            }
        }
        CjsExportKind::Named { is_void: true, .. } => vec![], // should have been dropped
        CjsExportKind::SelfRef => vec![],
    }
}

fn build_dropped_export_side_effect_items(span: Span, kind: CjsExportKind) -> Vec<ModuleItem> {
    let expr = match kind {
        CjsExportKind::ModuleExportsDefault { expr }
        | CjsExportKind::NamedDefault { expr }
        | CjsExportKind::Named {
            expr,
            is_void: false,
            ..
        } => expr,
        CjsExportKind::EsModuleFlag
        | CjsExportKind::ReExport { .. }
        | CjsExportKind::Named { is_void: true, .. }
        | CjsExportKind::DefaultMirror
        | CjsExportKind::SelfRef => return vec![],
    };

    vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt { span, expr }))]
}

// ============================================================
// Pre-pass: hoist require() calls out of complex expressions
// ============================================================

/// Hoists `require()` calls embedded inside sequence expressions and other
/// compound expressions into standalone statements so the classification
/// phase can convert them to ES imports.
///
/// Handles these patterns:
///
/// 1. `export default (i = require("./a.js"), require("./b.js"), expr)`
///    → `const i = require("./a.js"); require("./b.js"); export default expr;`
///
/// 2. `const a = (i = require("./a.js")) && i.__esModule ? i : { default: i }`
///    → `const i = require("./a.js"); const a = i;`
///    (inline conditional interop)
///
/// When every use of `a` is a read through `a.default` and `i` is private to
/// the helper expression, the pre-pass instead emits
/// `const a = require("./a.js").default` and rewrites those reads to `a`.
fn has_hoistable_require(items: &[ModuleItem], unresolved_mark: Mark) -> bool {
    items.iter().any(|item| match item {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export_default)) => {
            let expr = strip_parens(&export_default.expr);
            if let Expr::Seq(seq) = expr {
                if seq_has_require_call(&seq.exprs, unresolved_mark) {
                    return true;
                }
            }
            if let Expr::Call(outer_call) = expr {
                if let Callee::Expr(callee) = &outer_call.callee {
                    if let Expr::Call(inner_call) = strip_parens(callee) {
                        return is_require_call(inner_call, unresolved_mark).is_some();
                    }
                }
            }
            false
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) if var_decl.decls.len() == 1 => {
            var_decl.decls[0].init.as_ref().is_some_and(|init| {
                try_extract_inline_conditional_interop(init, unresolved_mark).is_some()
            }) || item_has_toplevel_require_named_member_arg(item, unresolved_mark)
        }
        _ => item_has_toplevel_require_named_member_arg(item, unresolved_mark),
    })
}

fn hoist_embedded_requires(module: &mut Module, unresolved_mark: Mark) {
    if !has_hoistable_require(&module.body, unresolved_mark) {
        return;
    }
    let default_only_interop_bindings =
        collect_default_only_inline_interop_bindings(module, unresolved_mark);
    if !default_only_interop_bindings.is_empty() {
        module.visit_mut_with(&mut DefaultInteropMemberRewriter {
            bindings: &default_only_interop_bindings,
        });
    }
    let mut new_body = Vec::with_capacity(module.body.len());
    let mut used_names = collect_all_declared_names(module);

    for item in std::mem::take(&mut module.body) {
        match &item {
            // Pattern 1: export default (seq_expr with require calls)
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export_default)) => {
                // Unwrap parens
                let expr = strip_parens(&export_default.expr);
                if let Expr::Seq(seq) = expr {
                    if seq_has_require_call(&seq.exprs, unresolved_mark) {
                        let (hoisted, final_expr) =
                            hoist_requires_from_seq(&seq.exprs, unresolved_mark, &mut used_names);
                        new_body.extend(hoisted);
                        new_body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(
                            ExportDefaultExpr {
                                span: export_default.span,
                                expr: final_expr,
                            },
                        )));
                        continue;
                    }
                }
                // Pattern: export default require("...")(args) — require then call
                // (Don't hoist plain `export default require("...")` — it's a valid
                // re-export that namespace_decomposition can see through.)
                if let Expr::Call(outer_call) = expr {
                    if let Callee::Expr(callee) = &outer_call.callee {
                        if let Expr::Call(inner_call) = strip_parens(callee) {
                            if is_require_call(inner_call, unresolved_mark).is_some() {
                                let local = make_ident(fresh_prefixed_name(
                                    &Atom::from("default"),
                                    &mut used_names,
                                ));
                                new_body.push(make_require_var_item(
                                    local.clone(),
                                    Box::new(Expr::Call(inner_call.clone())),
                                ));
                                let new_call = CallExpr {
                                    callee: Expr::Ident(local).as_callee(),
                                    args: outer_call.args.clone(),
                                    span: outer_call.span,
                                    ctxt: outer_call.ctxt,
                                    type_args: outer_call.type_args.clone(),
                                };
                                new_body.push(ModuleItem::ModuleDecl(
                                    ModuleDecl::ExportDefaultExpr(ExportDefaultExpr {
                                        span: export_default.span,
                                        expr: Box::new(Expr::Call(new_call)),
                                    }),
                                ));
                                continue;
                            }
                        }
                    }
                }
                new_body.push(item);
            }

            // Pattern 2: const a = (i = require("./a.js")) && i.__esModule ? i : { default: i }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) if var_decl.decls.len() == 1 => {
                let decl = &var_decl.decls[0];
                if let Some(init) = &decl.init {
                    if let Some((require_local, source_expr)) =
                        try_extract_inline_conditional_interop(init, unresolved_mark)
                    {
                        if let Pat::Ident(wrapper) = &decl.name {
                            if default_only_interop_bindings.contains(&binding_id(&wrapper.id)) {
                                let require_default = Box::new(Expr::Member(MemberExpr {
                                    span: DUMMY_SP,
                                    obj: source_expr,
                                    prop: MemberProp::Ident(IdentName::new(
                                        "default".into(),
                                        DUMMY_SP,
                                    )),
                                }));
                                new_body.push(make_require_var_item(
                                    wrapper.id.clone(),
                                    require_default,
                                ));
                                continue;
                            }
                        }
                        let (import_local, assign_after_require) =
                            import_local_for_assignment(require_local.clone(), &mut used_names);
                        new_body.push(make_require_var_item(import_local.clone(), source_expr));
                        if let Some(assign) = assign_after_require {
                            new_body.push(assign);
                        }
                        // Emit: const <binding> = <require_local>;
                        let new_decl = VarDeclarator {
                            init: Some(Box::new(Expr::Ident(require_local))),
                            ..decl.clone()
                        };
                        new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
                            decls: vec![new_decl],
                            ..*var_decl.clone()
                        })))));
                        continue;
                    }
                }
                new_body.push(item);
            }

            _ => new_body.push(item),
        }
    }

    module.body = new_body;
    hoist_toplevel_require_named_member_args(module, unresolved_mark);
}

/// Top-level immediately-evaluated Call whose direct argument is
/// `require("mod").Name` (not `.default`). Hoist to
/// `const Name = require("mod").Name` immediately before the statement so
/// the existing NamedProp classifier can emit `import { Name }`.
///
/// This pass is all-or-nothing: if any candidate fails the binding proof,
/// no argument is rewritten. Other UnEsm paths still run.
fn hoist_toplevel_require_named_member_args(module: &mut Module, unresolved_mark: Mark) {
    let candidates = collect_toplevel_require_named_member_args(&module.body, unresolved_mark);
    if candidates.is_empty() {
        return;
    }
    let Some(plan) = prove_toplevel_require_named_member_args(module, unresolved_mark, &candidates)
    else {
        return;
    };
    apply_toplevel_require_named_member_args(module, plan);
}

struct ToplevelRequireNamedMemberArg {
    item_index: usize,
    arg_index: usize,
    source: String,
    name: Atom,
}

struct ToplevelRequireNamedMemberInsert {
    local: Ident,
    init: Box<Expr>,
}

struct ToplevelRequireNamedMemberPlan {
    replacements: HashMap<usize, Vec<(usize, Ident)>>,
    inserts: HashMap<usize, Vec<ToplevelRequireNamedMemberInsert>>,
}

fn item_has_toplevel_require_named_member_arg(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let Some(call) = toplevel_item_call_expr(item) else {
        return false;
    };
    call.args.iter().any(|arg| {
        arg.spread.is_none()
            && match_require_named_member_expr(&arg.expr, unresolved_mark).is_some()
    })
}

fn collect_toplevel_require_named_member_args(
    items: &[ModuleItem],
    unresolved_mark: Mark,
) -> Vec<ToplevelRequireNamedMemberArg> {
    let mut candidates = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        let Some(call) = toplevel_item_call_expr(item) else {
            continue;
        };
        for (arg_index, arg) in call.args.iter().enumerate() {
            if arg.spread.is_some() {
                continue;
            }
            let Some((source, name)) = match_require_named_member_expr(&arg.expr, unresolved_mark)
            else {
                continue;
            };
            candidates.push(ToplevelRequireNamedMemberArg {
                item_index,
                arg_index,
                source,
                name,
            });
        }
    }
    candidates
}

fn has_toplevel_require_named_member_self_arg(
    items: &[ModuleItem],
    unresolved_mark: Mark,
    current_filename: Option<&str>,
) -> bool {
    let Some(current_filename) = current_filename else {
        return false;
    };
    let Some((normalized_filename, current_key)) = current_module_path_context(current_filename)
    else {
        return false;
    };

    collect_toplevel_require_named_member_args(items, unresolved_mark)
        .into_iter()
        .any(|candidate| {
            resolve_relative_specifier(&normalized_filename, &candidate.source).as_deref()
                == Some(&current_key)
        })
}

/// Direct argument of a top-level Call: `require("mod").Name` with a static
/// string specifier and a non-`default` ident (or computed ident string).
fn match_require_named_member_expr(expr: &Expr, unresolved_mark: Mark) -> Option<(String, Atom)> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    match_require_named_member(member, unresolved_mark)
}

fn match_require_named_member(
    member: &MemberExpr,
    unresolved_mark: Mark,
) -> Option<(String, Atom)> {
    let Expr::Call(call) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    let source = is_require_call(call, unresolved_mark)?;
    let prop = is_ident_prop(&member.prop)?;
    if prop.as_ref() == "default" {
        return None;
    }
    Some((source, prop))
}

/// Collect direct mutations of `require(source).Name`. Replacing separate
/// member reads with one recovered import is unsafe when the CommonJS object
/// can be changed through the same static edge.
struct RequireNamedMemberMutationCollector {
    unresolved_mark: Mark,
    write_target_depth: usize,
    mutations: HashSet<(String, Atom)>,
}

impl RequireNamedMemberMutationCollector {
    fn collect(module: &Module, unresolved_mark: Mark) -> HashSet<(String, Atom)> {
        let mut collector = Self {
            unresolved_mark,
            write_target_depth: 0,
            mutations: HashSet::new(),
        };
        module.visit_with(&mut collector);
        collector.mutations
    }
}

impl Visit for RequireNamedMemberMutationCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.write_target_depth > 0 {
            if let Some(mutation) = match_require_named_member(member, self.unresolved_mark) {
                self.mutations.insert(mutation);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        self.write_target_depth += 1;
        assignment.left.visit_with(self);
        self.write_target_depth -= 1;
        assignment.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        self.write_target_depth += 1;
        update.arg.visit_with(self);
        self.write_target_depth -= 1;
    }

    fn visit_unary_expr(&mut self, unary: &swc_core::ecma::ast::UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.write_target_depth += 1;
            unary.arg.visit_with(self);
            self.write_target_depth -= 1;
        } else {
            unary.visit_children_with(self);
        }
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.write_target_depth += 1;
        stmt.left.visit_with(self);
        self.write_target_depth -= 1;
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &swc_core::ecma::ast::ForOfStmt) {
        self.write_target_depth += 1;
        stmt.left.visit_with(self);
        self.write_target_depth -= 1;
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }
}

fn toplevel_item_call_expr(item: &ModuleItem) -> Option<&CallExpr> {
    let expr = match item {
        ModuleItem::Stmt(Stmt::Expr(stmt)) => stmt.expr.as_ref(),
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) if var.decls.len() == 1 => {
            var.decls[0].init.as_deref()?
        }
        _ => return None,
    };
    match strip_parens(expr) {
        Expr::Call(call) => Some(call),
        _ => None,
    }
}

fn toplevel_item_call_expr_mut(item: &mut ModuleItem) -> Option<&mut CallExpr> {
    match item {
        ModuleItem::Stmt(Stmt::Expr(stmt)) => call_expr_mut(stmt.expr.as_mut()),
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) if var.decls.len() == 1 => {
            call_expr_mut(var.decls[0].init.as_mut()?.as_mut())
        }
        _ => None,
    }
}

fn call_expr_mut(expr: &mut Expr) -> Option<&mut CallExpr> {
    match expr {
        Expr::Call(call) => Some(call),
        Expr::Paren(paren) => call_expr_mut(paren.expr.as_mut()),
        _ => None,
    }
}

fn existing_require_named_prop_item(
    item: &ModuleItem,
    source: &str,
    name: &Atom,
    unresolved_mark: Mark,
) -> Option<Ident> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    if binding.id.sym != *name {
        return None;
    }
    let init = decl.init.as_deref()?;
    match match_require_named_member_expr(init, unresolved_mark) {
        Some((ref existing_source, ref prop)) if existing_source == source && prop == name => {
            Some(binding.id.clone())
        }
        _ => None,
    }
}

/// Reuse a NamedProp local only when it is declared *before* the call, already
/// initialized, and never written. A later `var Name = require(src).Name` is
/// not a stable identity for an earlier argument; a mutated local is not the
/// original member read.
fn reusable_require_named_prop(
    items: &[ModuleItem],
    source: &str,
    name: &Atom,
    before_item_index: usize,
    unresolved_mark: Mark,
    uses: &BindingUseIndex,
) -> Option<Ident> {
    let mut reusable = None;
    for (index, item) in items.iter().enumerate() {
        if index >= before_item_index {
            break;
        }
        let Some(ident) = existing_require_named_prop_item(item, source, name, unresolved_mark)
        else {
            continue;
        };
        if uses.has_direct_write(&binding_id(&ident)) {
            return None;
        }
        reusable = Some(ident);
    }
    reusable
}

fn candidate_named_member_init(
    items: &[ModuleItem],
    candidate: &ToplevelRequireNamedMemberArg,
) -> Option<Box<Expr>> {
    let call = toplevel_item_call_expr(&items[candidate.item_index])?;
    let arg = call.args.get(candidate.arg_index)?;
    Some(Box::new(strip_parens(arg.expr.as_ref()).clone()))
}

fn prove_toplevel_require_named_member_args(
    module: &Module,
    unresolved_mark: Mark,
    candidates: &[ToplevelRequireNamedMemberArg],
) -> Option<ToplevelRequireNamedMemberPlan> {
    let mut eval_analyzer = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval_analyzer);
    if eval_analyzer.unknown_direct_eval {
        return None;
    }

    let declared_names = collect_all_declared_names(module);
    let unresolved_reference_names = collect_unresolved_reference_names(module, unresolved_mark);
    let uses = BindingUseIndex::collect(module);
    let provider_member_mutations =
        RequireNamedMemberMutationCollector::collect(module, unresolved_mark);
    let mut claimed_source_by_local: HashMap<Atom, String> = HashMap::new();
    let mut claimed_local: HashMap<Atom, Ident> = HashMap::new();
    let mut plan = ToplevelRequireNamedMemberPlan {
        replacements: HashMap::new(),
        inserts: HashMap::new(),
    };

    for candidate in candidates {
        let name_str = candidate.name.as_ref();
        if !is_valid_identifier_name(name_str) || is_reserved_binding_name(name_str) {
            return None;
        }
        if provider_member_mutations.contains(&(candidate.source.clone(), candidate.name.clone())) {
            return None;
        }
        if eval_analyzer
            .known_direct_eval_sources
            .iter()
            .any(|source| js_source_mentions_binding(source, &candidate.name))
        {
            return None;
        }

        let local = if let Some(claimed_source) = claimed_source_by_local.get(&candidate.name) {
            if claimed_source != &candidate.source {
                return None;
            }
            claimed_local
                .get(&candidate.name)
                .cloned()
                .expect("claimed source must have a local ident")
        } else if let Some(existing) = reusable_require_named_prop(
            &module.body,
            &candidate.source,
            &candidate.name,
            candidate.item_index,
            unresolved_mark,
            &uses,
        ) {
            claimed_source_by_local.insert(candidate.name.clone(), candidate.source.clone());
            claimed_local.insert(candidate.name.clone(), existing.clone());
            existing
        } else if declared_names.contains(&candidate.name)
            || unresolved_reference_names.contains(&candidate.name)
        {
            return None;
        } else {
            let local = make_ident(candidate.name.clone());
            claimed_source_by_local.insert(candidate.name.clone(), candidate.source.clone());
            claimed_local.insert(candidate.name.clone(), local.clone());
            let init = candidate_named_member_init(&module.body, candidate)?;
            plan.inserts.entry(candidate.item_index).or_default().push(
                ToplevelRequireNamedMemberInsert {
                    local: local.clone(),
                    init,
                },
            );
            local
        };

        plan.replacements
            .entry(candidate.item_index)
            .or_default()
            .push((candidate.arg_index, local));
    }

    Some(plan)
}

fn apply_toplevel_require_named_member_args(
    module: &mut Module,
    plan: ToplevelRequireNamedMemberPlan,
) {
    let mut new_body = Vec::with_capacity(module.body.len() + plan.inserts.len());
    for (index, mut item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        if let Some(inserts) = plan.inserts.get(&index) {
            for insert in inserts {
                new_body.push(make_require_var_item(
                    insert.local.clone(),
                    insert.init.clone(),
                ));
            }
        }
        if let Some(replacements) = plan.replacements.get(&index) {
            if let Some(call) = toplevel_item_call_expr_mut(&mut item) {
                for (arg_index, ident) in replacements {
                    if let Some(arg) = call.args.get_mut(*arg_index) {
                        *arg.expr = Expr::Ident(ident.clone());
                    }
                }
            }
        }
        new_body.push(item);
    }
    module.body = new_body;
}

/// Top-level immediately-evaluated Call whose direct argument is
/// `require("mod").default`. Hoist to `const L = require("mod").default`
/// immediately before the statement so the existing DefaultProp classifier
/// can emit `import L from "mod"`.
///
/// Independent of the named-member pass: a failed default proof must not
/// roll back named recovery, and vice versa. This pass is all-or-nothing
/// for `.default` arguments only.
fn hoist_toplevel_require_default_member_args(module: &mut Module, unresolved_mark: Mark) {
    let candidates = collect_toplevel_require_default_member_args(&module.body, unresolved_mark);
    if candidates.is_empty() {
        return;
    }
    let Some(plan) =
        prove_toplevel_require_default_member_args(module, unresolved_mark, &candidates)
    else {
        return;
    };
    apply_toplevel_require_default_member_args(module, plan);
}

struct ToplevelRequireDefaultMemberArg {
    item_index: usize,
    arg_index: usize,
    source: String,
}

struct ToplevelRequireDefaultMemberInsert {
    local: Ident,
    init: Box<Expr>,
}

struct ToplevelRequireDefaultMemberPlan {
    replacements: HashMap<usize, Vec<(usize, Ident)>>,
    inserts: HashMap<usize, Vec<ToplevelRequireDefaultMemberInsert>>,
}

fn collect_toplevel_require_default_member_args(
    items: &[ModuleItem],
    unresolved_mark: Mark,
) -> Vec<ToplevelRequireDefaultMemberArg> {
    let mut candidates = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        let Some(call) = toplevel_item_call_expr(item) else {
            continue;
        };
        for (arg_index, arg) in call.args.iter().enumerate() {
            if arg.spread.is_some() {
                continue;
            }
            let Some(source) = match_require_default_member_expr(&arg.expr, unresolved_mark) else {
                continue;
            };
            candidates.push(ToplevelRequireDefaultMemberArg {
                item_index,
                arg_index,
                source,
            });
        }
    }
    candidates
}

fn has_toplevel_require_default_member_self_arg(
    items: &[ModuleItem],
    unresolved_mark: Mark,
    current_filename: Option<&str>,
) -> bool {
    let Some(current_filename) = current_filename else {
        return false;
    };
    let Some((normalized_filename, current_key)) = current_module_path_context(current_filename)
    else {
        return false;
    };

    collect_toplevel_require_default_member_args(items, unresolved_mark)
        .into_iter()
        .any(|candidate| {
            resolve_relative_specifier(&normalized_filename, &candidate.source).as_deref()
                == Some(&current_key)
        })
}

/// Direct argument of a top-level Call: `require("mod").default` with a
/// static string specifier (Ident or computed ident string). Does not share
/// `match_require_named_member`, which must keep skipping `.default`.
fn match_require_default_member_expr(expr: &Expr, unresolved_mark: Mark) -> Option<String> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    match_require_default_member(member, unresolved_mark)
}

fn match_require_default_member(member: &MemberExpr, unresolved_mark: Mark) -> Option<String> {
    let Expr::Call(call) = strip_parens(member.obj.as_ref()) else {
        return None;
    };
    let source = is_require_call(call, unresolved_mark)?;
    let prop = is_ident_prop(&member.prop)?;
    if prop.as_ref() != "default" {
        return None;
    }
    Some(source)
}

/// Collect direct mutations of `require(source).default`. Folding later
/// default-member reads into one recovered import is unsafe when the
/// CommonJS object can be changed through the same static edge.
struct RequireDefaultMemberMutationCollector {
    unresolved_mark: Mark,
    write_target_depth: usize,
    mutations: HashSet<String>,
}

impl RequireDefaultMemberMutationCollector {
    fn collect(module: &Module, unresolved_mark: Mark) -> HashSet<String> {
        let mut collector = Self {
            unresolved_mark,
            write_target_depth: 0,
            mutations: HashSet::new(),
        };
        module.visit_with(&mut collector);
        collector.mutations
    }
}

impl Visit for RequireDefaultMemberMutationCollector {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if self.write_target_depth > 0 {
            if let Some(source) = match_require_default_member(member, self.unresolved_mark) {
                self.mutations.insert(source);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assignment: &AssignExpr) {
        self.write_target_depth += 1;
        assignment.left.visit_with(self);
        self.write_target_depth -= 1;
        assignment.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &swc_core::ecma::ast::UpdateExpr) {
        self.write_target_depth += 1;
        update.arg.visit_with(self);
        self.write_target_depth -= 1;
    }

    fn visit_unary_expr(&mut self, unary: &swc_core::ecma::ast::UnaryExpr) {
        if unary.op == UnaryOp::Delete {
            self.write_target_depth += 1;
            unary.arg.visit_with(self);
            self.write_target_depth -= 1;
        } else {
            unary.visit_children_with(self);
        }
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        self.write_target_depth += 1;
        stmt.left.visit_with(self);
        self.write_target_depth -= 1;
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, stmt: &swc_core::ecma::ast::ForOfStmt) {
        self.write_target_depth += 1;
        stmt.left.visit_with(self);
        self.write_target_depth -= 1;
        stmt.right.visit_with(self);
        stmt.body.visit_with(self);
    }
}

fn existing_require_default_prop_item(
    item: &ModuleItem,
    source: &str,
    unresolved_mark: Mark,
) -> Option<Ident> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let init = decl.init.as_deref()?;
    match match_require_default_member_expr(init, unresolved_mark) {
        Some(ref existing_source) if existing_source == source => Some(binding.id.clone()),
        _ => None,
    }
}

fn existing_default_import_item(item: &ModuleItem, source: &str) -> Option<Ident> {
    let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
        return None;
    };
    if import.type_only || wtf8_to_string(&import.src.value) != source {
        return None;
    }
    import.specifiers.iter().find_map(|spec| match spec {
        ImportSpecifier::Default(default) => Some(default.local.clone()),
        _ => None,
    })
}

/// Reuse a DefaultProp local or an existing default import only when it is
/// declared *before* the call and never written. A later
/// `var L = require(src).default` is not a stable identity for an earlier
/// argument; a mutated local is not the original member read.
fn reusable_require_default_local(
    items: &[ModuleItem],
    source: &str,
    before_item_index: usize,
    unresolved_mark: Mark,
    uses: &BindingUseIndex,
) -> Option<Ident> {
    let mut reusable = None;
    for (index, item) in items.iter().enumerate() {
        if index >= before_item_index {
            break;
        }
        let ident = existing_require_default_prop_item(item, source, unresolved_mark)
            .or_else(|| existing_default_import_item(item, source));
        let Some(ident) = ident else {
            continue;
        };
        if uses.has_direct_write(&binding_id(&ident)) {
            // A mutated earlier DefaultProp is not reusable, but a later
            // stable binding of the same source still is.
            continue;
        }
        reusable = Some(ident);
    }
    reusable
}

fn candidate_default_member_init(
    items: &[ModuleItem],
    candidate: &ToplevelRequireDefaultMemberArg,
) -> Option<Box<Expr>> {
    let call = toplevel_item_call_expr(&items[candidate.item_index])?;
    let arg = call.args.get(candidate.arg_index)?;
    Some(Box::new(strip_parens(arg.expr.as_ref()).clone()))
}

fn specifier_basename(source: &str) -> Option<Atom> {
    let last = source.rsplit('/').next().unwrap_or(source);
    let stem = last.strip_suffix(".js").unwrap_or(last);
    if stem.is_empty() {
        return None;
    }
    if !is_valid_identifier_name(stem) || is_reserved_binding_name(stem) {
        return None;
    }
    Some(Atom::from(stem))
}

fn direct_eval_mentions_name(analyzer: &DirectEvalAnalyzer, name: &Atom) -> bool {
    analyzer
        .known_direct_eval_sources
        .iter()
        .any(|source| js_source_mentions_binding(source, name))
}

fn fresh_default_import_name(used_names: &mut HashSet<Atom>) -> Atom {
    let base = Atom::from("defaultExport");
    if used_names.insert(base.clone()) {
        return base;
    }
    for suffix in 1usize.. {
        let candidate = Atom::from(format!("defaultExport_{suffix}"));
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn prove_toplevel_require_default_member_args(
    module: &Module,
    unresolved_mark: Mark,
    candidates: &[ToplevelRequireDefaultMemberArg],
) -> Option<ToplevelRequireDefaultMemberPlan> {
    let mut eval_analyzer = DirectEvalAnalyzer::default();
    module.visit_with(&mut eval_analyzer);
    if eval_analyzer.unknown_direct_eval {
        return None;
    }

    let declared_names = collect_all_declared_names(module);
    let unresolved_reference_names = collect_unresolved_reference_names(module, unresolved_mark);
    let uses = BindingUseIndex::collect(module);
    let provider_member_mutations =
        RequireDefaultMemberMutationCollector::collect(module, unresolved_mark);
    let mut used_names = declared_names;
    used_names.extend(unresolved_reference_names);
    let mut claimed_local_by_source: HashMap<String, Ident> = HashMap::new();
    let mut plan = ToplevelRequireDefaultMemberPlan {
        replacements: HashMap::new(),
        inserts: HashMap::new(),
    };
    for candidate in candidates {
        if provider_member_mutations.contains(&candidate.source) {
            return None;
        }

        let local = if let Some(existing) = claimed_local_by_source.get(&candidate.source) {
            existing.clone()
        } else if let Some(existing) = reusable_require_default_local(
            &module.body,
            &candidate.source,
            candidate.item_index,
            unresolved_mark,
            &uses,
        ) {
            claimed_local_by_source.insert(candidate.source.clone(), existing.clone());
            existing
        } else {
            let basename = specifier_basename(&candidate.source);
            let local_name = if let Some(ref name) = basename {
                if !used_names.contains(name) && !direct_eval_mentions_name(&eval_analyzer, name) {
                    used_names.insert(name.clone());
                    name.clone()
                } else {
                    let synthetic = fresh_default_import_name(&mut used_names);
                    if direct_eval_mentions_name(&eval_analyzer, &synthetic) {
                        return None;
                    }
                    synthetic
                }
            } else {
                let synthetic = fresh_default_import_name(&mut used_names);
                if direct_eval_mentions_name(&eval_analyzer, &synthetic) {
                    return None;
                }
                synthetic
            };
            let local = make_ident(local_name);
            claimed_local_by_source.insert(candidate.source.clone(), local.clone());
            let init = candidate_default_member_init(&module.body, candidate)?;
            plan.inserts.entry(candidate.item_index).or_default().push(
                ToplevelRequireDefaultMemberInsert {
                    local: local.clone(),
                    init,
                },
            );
            local
        };

        plan.replacements
            .entry(candidate.item_index)
            .or_default()
            .push((candidate.arg_index, local));
    }

    Some(plan)
}

fn apply_toplevel_require_default_member_args(
    module: &mut Module,
    plan: ToplevelRequireDefaultMemberPlan,
) {
    let mut new_body = Vec::with_capacity(module.body.len() + plan.inserts.len());
    for (index, mut item) in std::mem::take(&mut module.body).into_iter().enumerate() {
        if let Some(inserts) = plan.inserts.get(&index) {
            for insert in inserts {
                new_body.push(make_require_var_item(
                    insert.local.clone(),
                    insert.init.clone(),
                ));
            }
        }
        if let Some(replacements) = plan.replacements.get(&index) {
            if let Some(call) = toplevel_item_call_expr_mut(&mut item) {
                for (arg_index, ident) in replacements {
                    if let Some(arg) = call.args.get_mut(*arg_index) {
                        *arg.expr = Expr::Ident(ident.clone());
                    }
                }
            }
        }
        new_body.push(item);
    }
    module.body = new_body;
}

/// Find inline Babel interop wrappers that are observably just default-import
/// aliases. Both bindings must be closed over by the matched helper shape:
///
/// - every wrapper use is a read through `.default`; and
/// - the assigned require temp is a hoisted `var` or an earlier uninitialized
///   `let` with exactly the four uses proven by the matcher (assignment,
///   marker read, and both branches).
///
/// The second condition matters because replacing the helper also removes the
/// original `temp = require(...)` assignment.
fn collect_default_only_inline_interop_bindings(
    module: &Module,
    unresolved_mark: Mark,
) -> HashSet<BindingId> {
    let uses = BindingUseIndex::collect(module);
    let mut bindings = HashSet::new();

    for (item_idx, item) in module.body.iter().enumerate() {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            continue;
        };
        if var_decl.decls.len() != 1 {
            continue;
        }
        let decl = &var_decl.decls[0];
        let Pat::Ident(wrapper) = &decl.name else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        let Some((require_local, _)) =
            try_extract_inline_conditional_interop(init, unresolved_mark)
        else {
            continue;
        };

        let wrapper_id = binding_id(&wrapper.id);
        let require_id = binding_id(&require_local);
        if wrapper_id != require_id
            && uses.has_only_static_member_reads(&wrapper_id, "default")
            && has_available_uninitialized_temp(module, &require_id, item_idx)
            && uses.use_count(&require_id) == 4
        {
            bindings.insert(wrapper_id);
        }
    }

    bindings
}

fn has_available_uninitialized_temp(
    module: &Module,
    binding: &BindingId,
    use_item_idx: usize,
) -> bool {
    module.body.iter().enumerate().any(|(decl_item_idx, item)| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
            return false;
        };
        let binding_is_uninitialized = var_decl.decls.iter().any(|decl| {
            decl.init.is_none()
                && matches!(&decl.name, Pat::Ident(local) if binding_id(&local.id) == *binding)
        });

        binding_is_uninitialized
            && (var_decl.kind == VarDeclKind::Var
                || var_decl.kind == VarDeclKind::Let && decl_item_idx < use_item_idx)
    })
}

struct DefaultInteropMemberRewriter<'a> {
    bindings: &'a HashSet<BindingId>,
}

impl VisitMut for DefaultInteropMemberRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let member = match expr {
            Expr::Member(member) => member,
            Expr::OptChain(chain) => {
                let OptChainBase::Member(member) = chain.base.as_mut() else {
                    return;
                };
                member
            }
            _ => return,
        };
        let Expr::Ident(object) = member.obj.as_ref() else {
            return;
        };
        let MemberProp::Ident(property) = &member.prop else {
            return;
        };
        if property.sym.as_ref() == "default" && self.bindings.contains(&binding_id(object)) {
            *expr = Expr::Ident(object.clone());
        }
    }
}

/// Check if any expression in a sequence contains a require() call.
fn seq_has_require_call(exprs: &[Box<Expr>], unresolved_mark: Mark) -> bool {
    exprs
        .iter()
        .any(|expr| expr_contains_require(expr, unresolved_mark))
}

fn expr_contains_require(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Call(call) => {
            is_require_call(call, unresolved_mark).is_some()
                || match &call.callee {
                    Callee::Expr(callee) => expr_contains_require(callee, unresolved_mark),
                    _ => false,
                }
        }
        Expr::Assign(assign) => expr_contains_require(&assign.right, unresolved_mark),
        Expr::Paren(paren) => expr_contains_require(&paren.expr, unresolved_mark),
        Expr::Member(member) => expr_contains_require(&member.obj, unresolved_mark),
        _ => false,
    }
}

/// Hoist require() calls from a sequence expression.
///
/// For `(i = require("./a.js"), require("./b.js"), expr)`:
///   - `i = require("./a.js")` → `const i = require("./a.js");` (hoisted)
///   - `require("./b.js")` → `require("./b.js");` (hoisted as bare)
///   - `expr` → returned as the remaining expression
///
/// Returns (hoisted_items, final_expression).
fn hoist_requires_from_seq(
    exprs: &[Box<Expr>],
    unresolved_mark: Mark,
    used_names: &mut HashSet<Atom>,
) -> (Vec<ModuleItem>, Box<Expr>) {
    let mut hoisted = Vec::new();
    let mut remaining = Vec::new();

    for expr in exprs {
        let expr_ref = strip_parens(expr);

        // require("...") → bare import side-effect
        if let Expr::Call(call) = expr_ref {
            if is_require_call(call, unresolved_mark).is_some() {
                hoisted.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                    span: DUMMY_SP,
                    expr: expr.clone(),
                })));
                continue;
            }
        }

        // i = require("...") → const i = require("...")
        if let Expr::Assign(assign) = expr_ref {
            if assign.op == AssignOp::Assign {
                if let Some(target_ident) = simple_assign_target_ident(&assign.left) {
                    let right = strip_parens(&assign.right);
                    if let Expr::Call(call) = right {
                        if is_require_call(call, unresolved_mark).is_some() {
                            let (import_local, assign_after_require) =
                                import_local_for_assignment(target_ident.clone(), used_names);
                            hoisted.push(make_require_var_item(import_local, assign.right.clone()));
                            if let Some(assign) = assign_after_require {
                                hoisted.push(assign);
                            }
                            continue;
                        }
                    }
                }
            }
        }

        // Assignments whose right side contains require() deeper in the tree:
        // - c = i = require("...") → const c = i = require("...");
        // - a = (i = require("...")).lib → const a = (i = require("...")).lib;
        if let Expr::Assign(assign) = expr_ref {
            if assign.op == AssignOp::Assign {
                if let Some(outer_ident) = simple_assign_target_ident(&assign.left) {
                    if expr_contains_require(&assign.right, unresolved_mark) {
                        let (import_local, assign_after_require) =
                            import_local_for_assignment(outer_ident.clone(), used_names);
                        hoisted.push(make_var_item(import_local, assign.right.clone()));
                        if let Some(assign) = assign_after_require {
                            hoisted.push(assign);
                        }
                        continue;
                    }
                }
            }
        }

        remaining.push(expr.clone());
    }

    let final_expr = if remaining.is_empty() {
        Box::new(Expr::Ident(make_ident(Atom::from("undefined"))))
    } else if remaining.len() == 1 {
        remaining.into_iter().next().unwrap()
    } else {
        Box::new(Expr::Seq(SeqExpr {
            span: DUMMY_SP,
            exprs: remaining,
        }))
    };

    (hoisted, final_expr)
}

/// Match `(i = require("...")) && i.__esModule ? i : { default: i }`
/// Returns (require_local_ident, require_source_expr).
fn try_extract_inline_conditional_interop(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<(Ident, Box<Expr>)> {
    let expr = strip_parens(expr);

    // Must be: <test> ? <cons> : <alt>
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = expr
    else {
        return None;
    };

    // test must be: (i = require("...")) && i.__esModule
    // or: i && i.__esModule (where i was assigned in an outer sequence)
    let test = strip_parens(test);
    let Expr::Bin(bin) = test else {
        return None;
    };
    if bin.op != BinaryOp::LogicalAnd {
        return None;
    }

    // Right side must be: X.__esModule
    let right = strip_parens(&bin.right);
    let Expr::Member(member) = right else {
        return None;
    };
    let Expr::Ident(member_obj) = strip_parens(&member.obj) else {
        return None;
    };
    let MemberProp::Ident(IdentName { sym, .. }) = &member.prop else {
        return None;
    };
    if sym.as_ref() != "__esModule" {
        return None;
    }

    // Left side of && must contain the require assignment
    let left = strip_parens(&bin.left);

    // Pattern: (i = require("..."))
    if let Expr::Assign(assign) = left {
        if assign.op == AssignOp::Assign {
            if let Some(target) = simple_assign_target_ident(&assign.left) {
                let right_inner = strip_parens(&assign.right);
                if let Expr::Call(call) = right_inner {
                    if is_require_call(call, unresolved_mark).is_some() {
                        // Verify every interop branch refers to the same assigned binding.
                        if member_obj.sym == target.sym
                            && member_obj.ctxt == target.ctxt
                            && is_same_ident_ref(cons, &target)
                            && matches_default_object_for_ident(alt, &target)
                        {
                            return Some((target, assign.right.clone()));
                        }
                    }
                }
            }
        }
    }

    None
}

fn simple_assign_target_ident(target: &AssignTarget) -> Option<Ident> {
    if let AssignTarget::Simple(SimpleAssignTarget::Ident(bi)) = target {
        Some(bi.id.clone())
    } else {
        None
    }
}

fn is_same_ident_ref(expr: &Expr, ident: &Ident) -> bool {
    let expr = strip_parens(expr);
    if let Expr::Ident(id) = expr {
        id.sym == ident.sym && id.ctxt == ident.ctxt
    } else {
        false
    }
}

fn matches_default_object_for_ident(expr: &Expr, ident: &Ident) -> bool {
    let Expr::Object(obj) = strip_parens(expr) else {
        return false;
    };
    if obj.props.len() != 1 {
        return false;
    }
    let PropOrSpread::Prop(prop) = &obj.props[0] else {
        return false;
    };
    let Prop::KeyValue(kv) = prop.as_ref() else {
        return false;
    };
    let key_is_default = match &kv.key {
        PropName::Ident(id) => id.sym.as_ref() == "default",
        PropName::Str(s) => s.value.as_str() == Some("default"),
        _ => false,
    };
    key_is_default && is_same_ident_ref(&kv.value, ident)
}

fn import_local_for_assignment(
    target: Ident,
    used_names: &mut HashSet<Atom>,
) -> (Ident, Option<ModuleItem>) {
    if used_names.insert(target.sym.clone()) {
        return (target, None);
    }

    let temp = make_ident(fresh_prefixed_name(&target.sym, used_names));
    let assign = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
                id: target,
                type_ann: None,
            })),
            right: Box::new(Expr::Ident(temp.clone())),
        })),
    }));
    (temp, Some(assign))
}

fn make_require_var_item(local: Ident, require_expr: Box<Expr>) -> ModuleItem {
    make_var_item(local, require_expr)
}

fn make_var_item(local: Ident, init: Box<Expr>) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: Default::default(),
        kind: VarDeclKind::Const,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name: Pat::Ident(BindingIdent {
                id: local,
                type_ann: None,
            }),
            init: Some(init),
            definite: false,
        }],
    }))))
}

// ============================================================
// Classification helpers
// ============================================================

fn classify_item(
    item: ModuleItem,
    unresolved_mark: Mark,
    require_bindings: &HashMap<BindingId, String>,
) -> Classified {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Classified::ExistingImport(import),
        ModuleItem::Stmt(ref stmt) => {
            if let Some(kind) = try_classify_cjs_export(stmt, unresolved_mark, require_bindings) {
                let span = match stmt {
                    Stmt::Expr(expr_stmt) => expr_stmt.span,
                    _ => DUMMY_SP,
                };
                return Classified::CjsExport { span, kind };
            }
            if let Some(kind) = try_classify_cjs_require(stmt, unresolved_mark) {
                return Classified::CjsRequire(kind);
            }
            Classified::Keep(item)
        }
        other => Classified::Keep(other),
    }
}

fn collect_stable_require_bindings(
    module: &Module,
    uses: &BindingUseIndex,
    unresolved_mark: Mark,
) -> HashMap<BindingId, String> {
    let mut bindings = HashMap::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        if var.decls.len() != 1 {
            continue;
        }
        let declarator = &var.decls[0];
        let Pat::Ident(binding) = &declarator.name else {
            continue;
        };
        let Some(Expr::Call(call)) = declarator.init.as_deref().map(strip_parens) else {
            continue;
        };
        let Some(source) = is_require_call(call, unresolved_mark) else {
            continue;
        };
        let binding_id = (binding.id.sym.clone(), binding.id.ctxt);
        if uses.has_only_static_member_reads_any(&binding_id) {
            bindings.insert(binding_id, source);
        }
    }
    bindings
}

fn remove_default_export_mirrors(classified: &mut [Classified], unresolved_mark: Mark) {
    let mut saw_safe_named_default = false;
    for item in classified {
        if let Classified::CjsExport {
            span,
            kind: CjsExportKind::ModuleExportsDefault { expr },
        } = item
        {
            if saw_safe_named_default && is_exports_default_expr(expr.as_ref(), unresolved_mark) {
                let orig_span = *span;
                *item = Classified::CjsExport {
                    span: orig_span,
                    kind: CjsExportKind::DefaultMirror,
                };
            }
            saw_safe_named_default = false;
            continue;
        }

        if matches!(
            item,
            Classified::CjsExport {
                kind: CjsExportKind::NamedDefault { .. },
                ..
            }
        ) {
            saw_safe_named_default = true;
            continue;
        }

        if saw_safe_named_default && is_safe_intervening_default_mirror_item(item, unresolved_mark)
        {
            continue;
        }

        saw_safe_named_default = false;
    }
}

fn is_safe_intervening_default_mirror_item(item: &Classified, unresolved_mark: Mark) -> bool {
    match item {
        Classified::ExistingImport(_) | Classified::CjsRequire(_) => true,
        Classified::Keep(item) => is_safe_intervening_module_item(item, unresolved_mark),
        Classified::CjsExport { .. } => false,
    }
}

fn is_safe_intervening_module_item(item: &ModuleItem, unresolved_mark: Mark) -> bool {
    let mut finder = UnsafeDefaultMirrorInterveningFinder {
        unresolved_mark,
        found: false,
    };
    item.visit_with(&mut finder);
    !finder.found
}

struct UnsafeDefaultMirrorInterveningFinder {
    unresolved_mark: Mark,
    found: bool,
}

impl Visit for UnsafeDefaultMirrorInterveningFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if is_unresolved_ident(ident, "exports", self.unresolved_mark)
            || is_unresolved_ident(ident, "module", self.unresolved_mark)
        {
            self.found = true;
        }
    }

    fn visit_call_expr(&mut self, _: &CallExpr) {
        self.found = true;
    }

    fn visit_new_expr(&mut self, _: &swc_core::ecma::ast::NewExpr) {
        self.found = true;
    }

    fn visit_await_expr(&mut self, _: &swc_core::ecma::ast::AwaitExpr) {
        self.found = true;
    }

    fn visit_yield_expr(&mut self, _: &swc_core::ecma::ast::YieldExpr) {
        self.found = true;
    }

    fn visit_update_expr(&mut self, _: &swc_core::ecma::ast::UpdateExpr) {
        self.found = true;
    }

    fn visit_unary_expr(&mut self, expr: &swc_core::ecma::ast::UnaryExpr) {
        if expr.op == UnaryOp::Delete {
            self.found = true;
        } else {
            expr.visit_children_with(self);
        }
    }

    fn visit_assign_expr(&mut self, expr: &AssignExpr) {
        match &expr.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
                if is_unresolved_ident(&binding.id, "exports", self.unresolved_mark)
                    || is_unresolved_ident(&binding.id, "module", self.unresolved_mark) =>
            {
                self.found = true;
                return;
            }
            AssignTarget::Simple(SimpleAssignTarget::Ident(_)) => {}
            _ => {
                self.found = true;
                return;
            }
        }
        expr.right.visit_with(self);
    }

    fn visit_function(&mut self, _: &swc_core::ecma::ast::Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}

    fn visit_class(&mut self, _: &swc_core::ecma::ast::Class) {
        self.found = true;
    }
}

/// Split `(module.exports = factory)(args)` into a single-evaluation local,
/// an ordinary module export assignment, and the call. The assigned call may
/// be the receiver of a longer member/call chain, as in
/// `(module.exports = factory)(args).push(value)`, provided it remains the
/// root expression evaluated before the rest of the statement. The export
/// classifier can then lower the standalone assignment without losing the
/// call or leaving an unresolved CommonJS `module` reference in the processed
/// output.
fn split_called_module_exports_assignments(module: &mut Module, unresolved_mark: Mark) {
    let mut used_names = collect_all_identifier_names(module);
    let mut new_body = Vec::with_capacity(module.body.len());

    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Expr(mut expr_stmt)) = item else {
            new_body.push(item);
            continue;
        };
        let Some(assign) =
            called_module_exports_assignment_at_root(&expr_stmt.expr, unresolved_mark)
        else {
            new_body.push(ModuleItem::Stmt(Stmt::Expr(expr_stmt)));
            continue;
        };
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            new_body.push(ModuleItem::Stmt(Stmt::Expr(expr_stmt)));
            continue;
        };
        if assign.op != AssignOp::Assign || !is_module_exports_member(member, unresolved_mark) {
            new_body.push(ModuleItem::Stmt(Stmt::Expr(expr_stmt)));
            continue;
        }

        let value = assign.right.clone();
        let local = make_ident(fresh_prefixed_name(&Atom::from("default"), &mut used_names));
        let capture = ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: expr_stmt.span,
            ctxt: Default::default(),
            kind: VarDeclKind::Var,
            declare: false,
            decls: vec![VarDeclarator {
                span: expr_stmt.span,
                name: Pat::Ident(BindingIdent {
                    id: local.clone(),
                    type_ann: None,
                }),
                init: Some(value),
                definite: false,
            }],
        }))));
        let export = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: expr_stmt.span,
            expr: Box::new(Expr::Assign(AssignExpr {
                span: assign.span,
                op: AssignOp::Assign,
                left: assign.left.clone(),
                right: Box::new(Expr::Ident(local.clone())),
            })),
        }));
        let replaced = replace_called_module_exports_assignment_at_root(
            expr_stmt.expr.as_mut(),
            unresolved_mark,
            &local,
        );
        debug_assert!(replaced, "the immutable root match must remain replaceable");

        new_body.push(capture);
        new_body.push(export);
        new_body.push(ModuleItem::Stmt(Stmt::Expr(expr_stmt)));
    }

    module.body = new_body;
}

fn called_module_exports_assignment_at_root(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<&AssignExpr> {
    match strip_parens(expr) {
        Expr::Call(call) => {
            let callee = call.callee.as_expr()?;
            if let Expr::Assign(assign) = strip_parens(callee) {
                if assign.op == AssignOp::Assign
                    && matches!(
                        &assign.left,
                        AssignTarget::Simple(SimpleAssignTarget::Member(member))
                            if is_module_exports_member(member, unresolved_mark)
                    )
                {
                    return Some(assign);
                }
            }
            called_module_exports_assignment_at_root(callee, unresolved_mark)
        }
        Expr::Member(member) => {
            called_module_exports_assignment_at_root(&member.obj, unresolved_mark)
        }
        _ => None,
    }
}

fn replace_called_module_exports_assignment_at_root(
    expr: &mut Expr,
    unresolved_mark: Mark,
    local: &Ident,
) -> bool {
    match expr {
        Expr::Paren(paren) => replace_called_module_exports_assignment_at_root(
            paren.expr.as_mut(),
            unresolved_mark,
            local,
        ),
        Expr::Call(call) => {
            let Some(callee) = call.callee.as_mut_expr() else {
                return false;
            };
            let is_target = matches!(
                strip_parens(callee),
                Expr::Assign(assign)
                    if assign.op == AssignOp::Assign
                        && matches!(
                            &assign.left,
                            AssignTarget::Simple(SimpleAssignTarget::Member(member))
                                if is_module_exports_member(member, unresolved_mark)
                        )
            );
            if is_target {
                call.callee = Expr::Ident(local.clone()).as_callee();
                true
            } else {
                replace_called_module_exports_assignment_at_root(callee, unresolved_mark, local)
            }
        }
        Expr::Member(member) => replace_called_module_exports_assignment_at_root(
            member.obj.as_mut(),
            unresolved_mark,
            local,
        ),
        _ => false,
    }
}

/// Split a top-level `local = module.exports = expr` while preserving its
/// right-to-left assignment order. A fresh capture evaluates the RHS once,
/// then the ordinary export assignment runs before the original local write.
///
/// Only mutable module-level variable bindings are accepted. Unresolved
/// globals, imports, constants, member targets, and nested expression contexts
/// remain untouched rather than broadening this into a general expression
/// lowering pass.
fn split_chained_local_module_exports_assignments(module: &mut Module, unresolved_mark: Mark) {
    let mutable_bindings = collect_mutable_module_var_bindings(module);
    let mut used_names = collect_all_identifier_names(module);
    let mut new_body = Vec::with_capacity(module.body.len());

    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Expr(expr_stmt)) = item else {
            new_body.push(item);
            continue;
        };
        let Some((local_target, value)) = (|| {
            let Expr::Assign(local_assign) = strip_parens(expr_stmt.expr.as_ref()) else {
                return None;
            };
            if local_assign.op != AssignOp::Assign {
                return None;
            }
            let AssignTarget::Simple(SimpleAssignTarget::Ident(local)) = &local_assign.left else {
                return None;
            };
            if !mutable_bindings.contains(&binding_id(&local.id)) {
                return None;
            }
            let Expr::Assign(export_assign) = strip_parens(local_assign.right.as_ref()) else {
                return None;
            };
            if export_assign.op != AssignOp::Assign {
                return None;
            }
            let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &export_assign.left
            else {
                return None;
            };
            if !is_module_exports_member(member, unresolved_mark) {
                return None;
            }
            Some((local_assign.left.clone(), export_assign.right.clone()))
        })() else {
            new_body.push(ModuleItem::Stmt(Stmt::Expr(expr_stmt)));
            continue;
        };

        let local = make_ident(fresh_prefixed_name(&Atom::from("default"), &mut used_names));
        new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: expr_stmt.span,
            ctxt: Default::default(),
            kind: VarDeclKind::Var,
            declare: false,
            decls: vec![VarDeclarator {
                span: expr_stmt.span,
                name: Pat::Ident(BindingIdent {
                    id: local.clone(),
                    type_ann: None,
                }),
                init: Some(value),
                definite: false,
            }],
        })))));
        new_body.push(make_module_exports_assign_expr_item(
            expr_stmt.span,
            Box::new(Expr::Ident(local.clone())),
            unresolved_mark,
        ));
        new_body.push(ModuleItem::Stmt(Stmt::Expr(ExprStmt {
            span: expr_stmt.span,
            expr: Box::new(Expr::Assign(AssignExpr {
                span: expr_stmt.span,
                op: AssignOp::Assign,
                left: local_target,
                right: Box::new(Expr::Ident(local)),
            })),
        })));
    }

    module.body = new_body;
}

fn collect_mutable_module_var_bindings(module: &Module) -> HashSet<BindingId> {
    module
        .body
        .iter()
        .filter_map(|item| match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var)))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Var(var),
                ..
            })) if var.kind != VarDeclKind::Const => Some(var),
            _ => None,
        })
        .flat_map(|var| var.decls.iter().flat_map(|decl| find_pat_ids(&decl.name)))
        .collect()
}

/// Split compound export initializers so the normal export classification can
/// handle the extracted assignment:
///
/// - `var value = module.exports = expr` becomes
///   `var value = expr; module.exports = value`
/// - `var value = exports.X = expr` becomes
///   `var value = expr; exports.X = value`
///
/// The default-export form is restricted to a single declarator because moving
/// its export assignment past sibling initializers could change evaluation
/// order. The named-export path retains its existing multi-declarator support.
fn split_compound_exports(module: &mut Module, unresolved_mark: Mark) {
    let mut new_body = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var))) = item else {
            new_body.push(item);
            continue;
        };

        if var.decls.len() == 1 {
            let decl = &var.decls[0];
            if let (Pat::Ident(binding), Some(init)) = (&decl.name, &decl.init) {
                if let Some(real_init) =
                    try_extract_module_exports_assign(init.as_ref(), unresolved_mark)
                {
                    let mut new_var = (**var).clone();
                    new_var.decls[0].init = Some(real_init);
                    new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(new_var)))));
                    new_body.push(make_module_exports_assign_expr_item(
                        var.span,
                        Box::new(Expr::Ident(binding.id.clone())),
                        unresolved_mark,
                    ));
                    continue;
                }
            }
        }

        let mut any_split = false;
        let mut new_decls = Vec::new();
        let mut export_stmts = Vec::new();

        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                new_decls.push(decl.clone());
                continue;
            };
            let Some(init) = &decl.init else {
                new_decls.push(decl.clone());
                continue;
            };
            if let Some((export_name, real_init)) =
                try_extract_exports_assign(init, unresolved_mark)
            {
                any_split = true;
                // var s = expr (stripped of exports.X wrapper)
                new_decls.push(VarDeclarator {
                    init: Some(real_init),
                    ..decl.clone()
                });
                // exports.X = s
                export_stmts.push(Stmt::Expr(swc_core::ecma::ast::ExprStmt {
                    span: var.span,
                    expr: Box::new(Expr::Assign(swc_core::ecma::ast::AssignExpr {
                        span: DUMMY_SP,
                        op: AssignOp::Assign,
                        left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                            span: DUMMY_SP,
                            obj: Box::new(Expr::Ident(make_unresolved_ident(
                                "exports".into(),
                                unresolved_mark,
                            ))),
                            prop: MemberProp::Ident(IdentName::new(export_name, DUMMY_SP)),
                        })),
                        right: Box::new(Expr::Ident(binding.id.clone())),
                    })),
                }));
            } else {
                new_decls.push(decl.clone());
            }
        }

        if any_split {
            let mut new_var = (**var).clone();
            new_var.decls = new_decls;
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(new_var)))));
            for s in export_stmts {
                new_body.push(ModuleItem::Stmt(s));
            }
        } else {
            new_body.push(item);
        }
    }
    module.body = new_body;
}

fn try_extract_module_exports_assign(expr: &Expr, unresolved_mark: Mark) -> Option<Box<Expr>> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    is_module_exports_member(member, unresolved_mark).then(|| assign.right.clone())
}

fn make_module_exports_assign_expr_item(
    span: Span,
    expr: Box<Expr>,
    unresolved_mark: Mark,
) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(Expr::Ident(make_unresolved_ident(
                    "module".into(),
                    unresolved_mark,
                ))),
                prop: MemberProp::Ident(IdentName::new("exports".into(), DUMMY_SP)),
            })),
            right: expr,
        })),
    }))
}

/// Lower `export const dep = require("dep")` into
/// `const dep = require("dep"); export { dep };` so the normal require
/// classifier can convert the declaration into an import while preserving the
/// exported binding.
fn lower_exported_cjs_requires(module: &mut Module, unresolved_mark: Mark) {
    let mut new_body = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
            span,
            decl: Decl::Var(var),
        })) = item
        else {
            new_body.push(item);
            continue;
        };

        let has_require_decl = var
            .decls
            .iter()
            .any(|decl| try_classify_cjs_require_declarator(decl, unresolved_mark).is_some());
        if !has_require_decl {
            new_body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span,
                decl: Decl::Var(var),
            })));
            continue;
        }

        let VarDecl {
            span: var_span,
            ctxt,
            kind,
            declare,
            decls,
        } = *var;

        for decl in decls {
            let single_decl = VarDecl {
                span: var_span,
                ctxt,
                kind,
                declare,
                decls: vec![decl.clone()],
            };
            if try_classify_cjs_require_declarator(&decl, unresolved_mark).is_some() {
                let specifiers = export_specifiers_for_pat(&decl.name);
                new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(
                    single_decl,
                )))));
                if !specifiers.is_empty() {
                    new_body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
                        NamedExport {
                            span,
                            specifiers,
                            src: None,
                            type_only: false,
                            with: None,
                        },
                    )));
                }
            } else {
                new_body.push(ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                    span,
                    decl: Decl::Var(Box::new(single_decl)),
                })));
            }
        }
    }
    module.body = new_body;
}

/// Keep writes to a CommonJS require local separate from the immutable ESM
/// import binding that will replace the require declaration. This includes an
/// authored `const`: its later write must keep throwing on that local binding.
///
/// For example, `var dependency = require("dep"); dependency = next` becomes
/// `_dependency = require("dep"); var dependency = _dependency; ...` before
/// classification. The ordinary require conversion then turns only the fresh
/// capture into an import. Object patterns use the same whole-value capture so
/// the original destructuring binding remains local.
fn preserve_written_cjs_require_bindings(module: &mut Module, unresolved_mark: Mark) {
    let uses = BindingUseIndex::collect(module);
    let mut used_names = collect_all_identifier_names(module);
    let mut new_body = Vec::with_capacity(module.body.len());

    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(mut var))) = item else {
            new_body.push(item);
            continue;
        };

        // A write to a `const` binding is already invalid in the input. Keep
        // that authored local around the synthesized import as well, so the
        // failure remains attached to the original declaration contract.
        if var.decls.len() != 1 {
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
            continue;
        }

        let declarator = &mut var.decls[0];
        if try_classify_cjs_require_declarator(declarator, unresolved_mark).is_none() {
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
            continue;
        }

        let binding_ids = find_pat_ids(&declarator.name);
        if !binding_ids.iter().any(|id| uses.has_direct_write(id)) {
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
            continue;
        }

        let Some(init) = declarator.init.take() else {
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
            continue;
        };
        let Some((base_name, _)) = binding_ids.first() else {
            declarator.init = Some(init);
            new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
            continue;
        };

        let import_local = make_ident(fresh_prefixed_name(base_name, &mut used_names));
        declarator.init = Some(Box::new(Expr::Ident(import_local.clone())));

        let capture = VarDecl {
            span: var.span,
            ctxt: var.ctxt,
            kind: var.kind,
            declare: false,
            decls: vec![VarDeclarator {
                span: declarator.span,
                name: Pat::Ident(BindingIdent {
                    id: import_local,
                    type_ann: None,
                }),
                init: Some(init),
                definite: false,
            }],
        };
        new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(capture)))));
        new_body.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
    }

    module.body = new_body;
}

fn export_specifiers_for_pat(pat: &Pat) -> Vec<ExportSpecifier> {
    find_pat_ids(pat)
        .into_iter()
        .map(|(sym, ctxt)| {
            ExportSpecifier::Named(ExportNamedSpecifier {
                span: DUMMY_SP,
                orig: ModuleExportName::Ident(Ident::new(sym, DUMMY_SP, ctxt)),
                exported: None,
                is_type_only: false,
            })
        })
        .collect()
}

/// Extract `exports.X` from `exports.X = expr`, returning `(X, expr)`.
fn try_extract_exports_assign(expr: &Expr, unresolved_mark: Mark) -> Option<(Atom, Box<Expr>)> {
    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };
    let Expr::Ident(obj_id) = member.obj.as_ref() else {
        return None;
    };
    if !is_unresolved_ident(obj_id, "exports", unresolved_mark) {
        return None;
    }
    let prop_name = is_ident_prop(&member.prop)?;
    Some((prop_name, assign.right.clone()))
}

/// Try to classify as a CJS export statement
fn try_classify_cjs_export(
    stmt: &Stmt,
    unresolved_mark: Mark,
    require_bindings: &HashMap<BindingId, String>,
) -> Option<CjsExportKind> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    if let Some(kind) = try_classify_define_property_export(
        expr_stmt.expr.as_ref(),
        unresolved_mark,
        require_bindings,
    ) {
        return Some(kind);
    }

    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };

    // Must be simple `=` assignment (not +=, etc.)
    if assign.op != AssignOp::Assign {
        return None;
    }

    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
        return None;
    };

    // Check if obj is `module.exports` or `exports`.
    if is_cjs_export_object_expr(&member.obj, unresolved_mark) {
        if let Some(prop) = is_ident_prop(&member.prop) {
            if prop.as_ref() == "default" {
                // module.exports.default = module.exports → self-ref
                if is_module_exports_expr(&assign.right, unresolved_mark) {
                    return Some(CjsExportKind::SelfRef);
                }
                return Some(CjsExportKind::NamedDefault {
                    expr: assign.right.clone(),
                });
            }
            // `exports.__proto__ = value` triggers the prototype setter and
            // creates no own property, so an `export const __proto__` both
            // fabricates an export and erases the prototype change. Keep the
            // assignment as an honest CommonJS residual.
            if is_prototype_mutating_member_name(prop.as_ref()) {
                return None;
            }
            let is_void = is_void_or_undefined(&assign.right, unresolved_mark);
            return Some(CjsExportKind::Named {
                name: prop,
                expr: assign.right.clone(),
                is_void,
            });
        }
        // bracket notation on module.exports — skip
        return None;
    }

    // Check if member is exactly `module.exports` (obj=module, prop=exports)
    if let Expr::Ident(obj_id) = member.obj.as_ref() {
        if is_unresolved_ident(obj_id, "module", unresolved_mark) {
            if let MemberProp::Ident(IdentName { sym, .. }) = &member.prop {
                if sym.as_ref() == "exports" {
                    // module.exports = expr (module.exports as an assignment target)
                    return Some(CjsExportKind::ModuleExportsDefault {
                        expr: assign.right.clone(),
                    });
                }
            }
            // module["exports"] = expr — skip (bracket notation)
            return None;
        }
    }

    None
}

fn try_classify_define_property_export(
    expr: &Expr,
    unresolved_mark: Mark,
    require_bindings: &HashMap<BindingId, String>,
) -> Option<CjsExportKind> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if !is_object_define_property_global_call(call, unresolved_mark) || call.args.len() != 3 {
        return None;
    }
    if !is_cjs_export_object_expr(call.args[0].expr.as_ref(), unresolved_mark) {
        return None;
    }
    if is_esmodule_name_arg(call.args[1].expr.as_ref())
        && is_esmodule_descriptor(call.args[2].expr.as_ref())
    {
        return Some(CjsExportKind::EsModuleFlag);
    }

    let export_name = literal_export_name_arg(call.args[1].expr.as_ref())?;
    if let Some(ident) =
        extract_define_property_getter_ident(call.args[2].expr.as_ref(), unresolved_mark)
    {
        return Some(CjsExportKind::Named {
            name: export_name,
            expr: Box::new(Expr::Ident(ident)),
            is_void: false,
        });
    }

    let (base, imported) = extract_define_property_getter_member(call.args[2].expr.as_ref())?;
    let binding = (base.sym.clone(), base.ctxt);
    let source = require_bindings.get(&binding)?.clone();
    Some(CjsExportKind::ReExport {
        name: export_name,
        imported,
        source,
        binding,
    })
}

/// Try to classify as a CJS require statement
fn try_classify_cjs_require(stmt: &Stmt, unresolved_mark: Mark) -> Option<CjsRequireKind> {
    match stmt {
        // Bare require: require('foo');
        Stmt::Expr(expr_stmt) => {
            if let Expr::Call(call) = expr_stmt.expr.as_ref() {
                if let Some(source) = is_require_call(call, unresolved_mark) {
                    return Some(CjsRequireKind::Bare { source });
                }
            }
            None
        }
        // var ... = require(...)[...]
        Stmt::Decl(Decl::Var(var)) => {
            // Must be a single declarator
            if var.decls.len() != 1 {
                return None;
            }
            try_classify_cjs_require_declarator(&var.decls[0], unresolved_mark)
        }
        _ => None,
    }
}

fn try_classify_cjs_require_declarator(
    decl: &VarDeclarator,
    unresolved_mark: Mark,
) -> Option<CjsRequireKind> {
    let Some(init) = &decl.init else { return None };

    match &decl.name {
        Pat::Ident(binding) => {
            let local = binding.id.clone();
            // var foo = require('bar')
            if let Expr::Call(call) = init.as_ref() {
                if let Some(source) = is_require_call(call, unresolved_mark) {
                    return Some(CjsRequireKind::Default { local, source });
                }
            }
            // var foo = require('bar').baz or require('bar').default
            if let Expr::Member(member) = init.as_ref() {
                if let Expr::Call(call) = member.obj.as_ref() {
                    if let Some(source) = is_require_call(call, unresolved_mark) {
                        if let Some(prop) = is_ident_prop(&member.prop) {
                            if prop.as_ref() == "default" {
                                return Some(CjsRequireKind::DefaultProp { local, source });
                            } else {
                                return Some(CjsRequireKind::NamedProp {
                                    prop,
                                    local,
                                    source,
                                });
                            }
                        }
                        // Invalid ident prop or bracket notation → skip
                        return None;
                    }
                }
            }
            None
        }
        Pat::Object(obj_pat) => {
            // var { a, b: c } = require('foo')
            if let Expr::Call(call) = init.as_ref() {
                if let Some(source) = is_require_call(call, unresolved_mark) {
                    let mut specifiers: Vec<(Atom, Ident)> = Vec::new();
                    for prop in &obj_pat.props {
                        match prop {
                            ObjectPatProp::KeyValue(kv) => {
                                // { b: c } → import { b as c }
                                let imported = match &kv.key {
                                    swc_core::ecma::ast::PropName::Ident(i) => i.sym.clone(),
                                    swc_core::ecma::ast::PropName::Str(s) => {
                                        Atom::from(s.value.as_str().unwrap_or(""))
                                    }
                                    _ => return None,
                                };
                                let local = extract_binding_ident(&kv.value)?;
                                specifiers.push((imported, local));
                            }
                            ObjectPatProp::Assign(a) => {
                                // { foo } → import { foo }
                                let ident = a.key.id.clone();
                                let name = ident.sym.clone();
                                specifiers.push((name, ident));
                            }
                            ObjectPatProp::Rest(_) => {
                                // rest spread — skip transformation
                                return None;
                            }
                        }
                    }
                    return Some(CjsRequireKind::Named { specifiers, source });
                }
            }
            // var { bar } = require('foo').baz — complex, skip
            None
        }
        _ => None,
    }
}

fn extract_binding_ident(pat: &Pat) -> Option<Ident> {
    match pat {
        Pat::Ident(bi) => Some(bi.id.clone()),
        Pat::Assign(a) => extract_binding_ident(&a.left),
        _ => None,
    }
}

// ============================================================
// Helper functions
// ============================================================

/// Check if call is `require('...')` and return the source string
fn is_require_call(call: &CallExpr, unresolved_mark: Mark) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if !is_unresolved_ident(id, "require", unresolved_mark) {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    let arg = &call.args[0];
    if arg.spread.is_some() {
        return None;
    }
    if let Expr::Lit(Lit::Str(s)) = arg.expr.as_ref() {
        Some(s.value.as_str().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Check if prop is an identifier (dot notation) and return name
/// Also accepts computed access with a valid JS identifier string literal
fn is_ident_prop(prop: &MemberProp) -> Option<Atom> {
    match prop {
        MemberProp::Ident(ident_name) => Some(ident_name.sym.clone()),
        MemberProp::Computed(computed) => {
            if let Expr::Lit(Lit::Str(s)) = computed.expr.as_ref() {
                let s_str = s.value.as_str()?;
                if is_valid_js_ident(s_str) {
                    Some(Atom::from(s_str))
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check if expr is `void N` or `undefined`
fn is_void_or_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::Void => true,
        Expr::Ident(id) if is_undefined_ident(id, unresolved_mark) => true,
        _ => false,
    }
}

/// Check if expr is `module.exports`
fn is_module_exports_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    if let Expr::Member(MemberExpr { obj, prop, .. }) = expr {
        if let Expr::Ident(id) = obj.as_ref() {
            if is_unresolved_ident(id, "module", unresolved_mark) {
                if let MemberProp::Ident(IdentName { sym, .. }) = prop {
                    return sym.as_ref() == "exports";
                }
            }
        }
    }
    false
}

fn is_cjs_export_object_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let expr = strip_parens(expr);
    if is_module_exports_expr(expr, unresolved_mark) {
        return true;
    }
    let Expr::Ident(id) = expr else {
        return false;
    };
    is_unresolved_ident(id, "exports", unresolved_mark)
}

fn is_object_define_property_global_call(call: &CallExpr, unresolved_mark: Mark) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    is_unresolved_member_expr(callee.as_ref(), "Object", "defineProperty", unresolved_mark)
}

fn is_esmodule_name_arg(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Lit(Lit::Str(str)) if str.value.as_str() == Some("__esModule"))
}

fn literal_export_name_arg(expr: &Expr) -> Option<Atom> {
    let Expr::Lit(Lit::Str(str)) = strip_parens(expr) else {
        return None;
    };
    let value = str.value.as_str()?;
    if is_valid_js_ident(value) {
        Some(value.into())
    } else {
        None
    }
}

fn is_esmodule_descriptor(expr: &Expr) -> bool {
    let Expr::Object(object) = strip_parens(expr) else {
        return false;
    };
    let mut has_value_true = false;
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        let Prop::KeyValue(entry) = prop.as_ref() else {
            return false;
        };
        let Some(name) = prop_name_as_atom(&entry.key) else {
            return false;
        };
        match name.as_ref() {
            "value" => {
                if !matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(value)) if value.value) {
                    return false;
                }
                has_value_true = true;
            }
            "enumerable" | "configurable" | "writable" => {
                if !matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(_))) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    has_value_true
}

fn extract_define_property_getter_ident(expr: &Expr, unresolved_mark: Mark) -> Option<Ident> {
    let expr = extract_define_property_getter_expr(expr)?;
    let Expr::Ident(ident) = expr.as_ref() else {
        return None;
    };
    if ident.ctxt.outer() == unresolved_mark {
        return None;
    }
    Some(ident.clone())
}

fn extract_define_property_getter_member(expr: &Expr) -> Option<(Ident, Atom)> {
    let expr = extract_define_property_getter_expr(expr)?;
    let Expr::Member(member) = strip_parens(&expr) else {
        return None;
    };
    let Expr::Ident(base) = strip_parens(&member.obj) else {
        return None;
    };
    Some((base.clone(), is_ident_prop(&member.prop)?))
}

fn extract_define_property_getter_expr(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Object(object) = strip_parens(expr) else {
        return None;
    };
    let mut has_enumerable_true = false;
    let mut has_enumerable = false;
    let mut has_configurable = false;
    let mut getter_expr = None;

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        match prop.as_ref() {
            Prop::KeyValue(entry) => match prop_name_as_atom(&entry.key).as_deref() {
                Some("enumerable") => {
                    if has_enumerable || !matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(_))) {
                        return None;
                    }
                    has_enumerable = true;
                    has_enumerable_true =
                        matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(value)) if value.value);
                }
                Some("get") => {
                    if getter_expr.is_some() {
                        return None;
                    }
                    getter_expr = Some(extract_getter_expr_return_expr(entry.value.as_ref())?);
                }
                Some("configurable") => {
                    if has_configurable {
                        return None;
                    }
                    if !matches!(entry.value.as_ref(), Expr::Lit(Lit::Bool(_))) {
                        return None;
                    }
                    has_configurable = true;
                }
                _ => return None,
            },
            Prop::Method(method) => {
                if matches!(prop_name_as_atom(&method.key).as_deref(), Some("get")) {
                    if getter_expr.is_some() {
                        return None;
                    }
                    if !method.function.params.is_empty()
                        || method.function.is_async
                        || method.function.is_generator
                    {
                        return None;
                    }
                    getter_expr = Some(extract_single_return_expr(method.function.body.as_ref()?)?);
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }

    has_enumerable_true.then_some(getter_expr).flatten()
}

fn is_unresolved_ident(id: &Ident, name: &str, unresolved_mark: Mark) -> bool {
    id.sym.as_ref() == name && id.ctxt.outer() == unresolved_mark
}

fn is_undefined_ident(id: &Ident, unresolved_mark: Mark) -> bool {
    id.sym.as_ref() == "undefined"
        && (id.ctxt.outer() == unresolved_mark || id.ctxt == SyntaxContext::empty())
}

/// Check if a string is a valid JS identifier
fn is_valid_js_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn make_str(value: &str) -> Str {
    Str {
        span: DUMMY_SP,
        value: value.into(),
        raw: None,
    }
}

fn make_ident(sym: Atom) -> Ident {
    Ident::new_no_ctxt(sym, DUMMY_SP)
}

fn make_unresolved_ident(sym: Atom, unresolved_mark: Mark) -> Ident {
    Ident::new(
        sym,
        DUMMY_SP,
        SyntaxContext::empty().apply_mark(unresolved_mark),
    )
}

/// Extract the span from a ModuleItem. Returns DUMMY_SP for items without
/// a meaningful span (e.g. synthesized items).
fn module_item_span(item: &ModuleItem) -> Span {
    match item {
        ModuleItem::Stmt(stmt) => match stmt {
            Stmt::Expr(expr_stmt) => expr_stmt.span,
            Stmt::Decl(Decl::Var(var)) => var.span,
            Stmt::Decl(Decl::Fn(f)) => f.function.span,
            _ => DUMMY_SP,
        },
        ModuleItem::ModuleDecl(decl) => match decl {
            ModuleDecl::Import(i) => i.span,
            ModuleDecl::ExportDecl(e) => e.span,
            ModuleDecl::ExportNamed(e) => e.span,
            ModuleDecl::ExportDefaultExpr(e) => e.span,
            ModuleDecl::ExportDefaultDecl(e) => e.span,
            ModuleDecl::ExportAll(e) => e.span,
            _ => DUMMY_SP,
        },
    }
}

fn wtf8_to_string(value: &swc_core::atoms::Wtf8Atom) -> String {
    value.as_str().unwrap_or("").to_string()
}

fn collect_all_declared_names(module: &Module) -> HashSet<Atom> {
    struct Collector {
        names: HashSet<Atom>,
    }

    impl Visit for Collector {
        fn visit_pat(&mut self, pat: &Pat) {
            collect_pat_names(pat, &mut self.names);
            pat.visit_children_with(self);
        }

        fn visit_import_decl(&mut self, import: &ImportDecl) {
            for spec in &import.specifiers {
                match spec {
                    ImportSpecifier::Named(named) => {
                        self.names.insert(named.local.sym.clone());
                    }
                    ImportSpecifier::Default(default) => {
                        self.names.insert(default.local.sym.clone());
                    }
                    ImportSpecifier::Namespace(namespace) => {
                        self.names.insert(namespace.local.sym.clone());
                    }
                }
            }
        }

        fn visit_decl(&mut self, decl: &Decl) {
            match decl {
                Decl::Fn(function) => {
                    self.names.insert(function.ident.sym.clone());
                    function.function.visit_with(self);
                }
                Decl::Class(class) => {
                    self.names.insert(class.ident.sym.clone());
                    class.class.visit_with(self);
                }
                _ => decl.visit_children_with(self),
            }
        }
    }

    let mut collector = Collector {
        names: collect_module_names(module),
    };
    module.visit_with(&mut collector);
    collector.names
}

fn collect_all_identifier_names(module: &Module) -> HashSet<Atom> {
    struct Collector {
        names: HashSet<Atom>,
    }

    impl Visit for Collector {
        fn visit_ident(&mut self, ident: &Ident) {
            self.names.insert(ident.sym.clone());
        }
    }

    let mut collector = Collector {
        names: HashSet::new(),
    };
    module.visit_with(&mut collector);
    collector.names
}

fn collect_conflicting_import_renames(
    items: &[ModuleItem],
    conflicts: &HashSet<Atom>,
    used_names: &mut HashSet<Atom>,
    renames: &mut Vec<BindingRename>,
) {
    for item in items {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            continue;
        };
        for spec in &import.specifiers {
            let local = match spec {
                ImportSpecifier::Named(named) => &named.local,
                ImportSpecifier::Default(default) => &default.local,
                ImportSpecifier::Namespace(namespace) => &namespace.local,
            };
            collect_conflicting_ident_rename(local, conflicts, used_names, renames);
        }
    }
}

fn collect_conflicting_decl_renames(
    decl: &Decl,
    conflicts: &HashSet<Atom>,
    used_names: &mut HashSet<Atom>,
    renames: &mut Vec<BindingRename>,
) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls {
                collect_conflicting_pat_renames(&d.name, conflicts, used_names, renames);
            }
        }
        Decl::Fn(f) => collect_conflicting_ident_rename(&f.ident, conflicts, used_names, renames),
        Decl::Class(c) => {
            collect_conflicting_ident_rename(&c.ident, conflicts, used_names, renames)
        }
        _ => {}
    }
}

fn collect_conflicting_pat_renames(
    pat: &Pat,
    conflicts: &HashSet<Atom>,
    used_names: &mut HashSet<Atom>,
    renames: &mut Vec<BindingRename>,
) {
    match pat {
        Pat::Ident(id) => collect_conflicting_ident_rename(&id.id, conflicts, used_names, renames),
        Pat::Array(arr) => {
            for p in arr.elems.iter().flatten() {
                collect_conflicting_pat_renames(p, conflicts, used_names, renames);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => {
                        collect_conflicting_pat_renames(&kv.value, conflicts, used_names, renames);
                    }
                    ObjectPatProp::Assign(a) => {
                        collect_conflicting_ident_rename(&a.key.id, conflicts, used_names, renames);
                    }
                    ObjectPatProp::Rest(r) => {
                        collect_conflicting_pat_renames(&r.arg, conflicts, used_names, renames);
                    }
                }
            }
        }
        Pat::Assign(a) => collect_conflicting_pat_renames(&a.left, conflicts, used_names, renames),
        Pat::Rest(r) => collect_conflicting_pat_renames(&r.arg, conflicts, used_names, renames),
        _ => {}
    }
}

fn collect_conflicting_ident_rename(
    ident: &Ident,
    conflicts: &HashSet<Atom>,
    used_names: &mut HashSet<Atom>,
    renames: &mut Vec<BindingRename>,
) {
    if !conflicts.contains(&ident.sym) {
        return;
    }
    let new = fresh_prefixed_name(&ident.sym, used_names);
    renames.push(BindingRename {
        old: (ident.sym.clone(), ident.ctxt),
        new,
    });
}

fn fresh_prefixed_name(name: &Atom, used_names: &mut HashSet<Atom>) -> Atom {
    let base = format!("_{name}");
    let atom = Atom::from(base);
    if used_names.insert(atom.clone()) {
        return atom;
    }

    let mut index = 2usize;
    loop {
        let candidate = Atom::from(format!("_{name}_{index}"));
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn rename_export_kind(kind: &mut CjsExportKind, renames: &[BindingRename]) {
    match kind {
        CjsExportKind::ModuleExportsDefault { expr }
        | CjsExportKind::Named { expr, .. }
        | CjsExportKind::NamedDefault { expr } => {
            rename_bindings(expr.as_mut(), renames);
        }
        CjsExportKind::EsModuleFlag
        | CjsExportKind::ReExport { .. }
        | CjsExportKind::DefaultMirror
        | CjsExportKind::SelfRef => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_import_fallback_uses_delimited_suffixes() {
        let mut used_names = HashSet::new();

        assert_eq!(fresh_default_import_name(&mut used_names), "defaultExport");
        assert_eq!(
            fresh_default_import_name(&mut used_names),
            "defaultExport_1"
        );
        assert_eq!(
            fresh_default_import_name(&mut used_names),
            "defaultExport_2"
        );
    }

    #[test]
    fn prefixed_name_uses_delimited_suffix() {
        let mut used_names = HashSet::from([Atom::from("_value")]);

        assert_eq!(
            fresh_prefixed_name(&Atom::from("value"), &mut used_names),
            "_value_2"
        );
    }

    #[test]
    fn get_or_insert_records_source_order_once() {
        let mut order = Vec::new();
        let mut map = HashMap::new();

        get_or_insert(&mut order, &mut map, "react".to_string());
        get_or_insert(&mut order, &mut map, "react".to_string());
        get_or_insert(&mut order, &mut map, "lodash".to_string());

        assert_eq!(order, vec!["react", "lodash"]);
        assert!(map.contains_key("react"));
        assert!(map.contains_key("lodash"));
    }
}
