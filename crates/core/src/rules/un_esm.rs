use std::collections::{hash_map::Entry, HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent, BlockStmt,
    BlockStmtOrExpr, CallExpr, Callee, CondExpr, Decl, ExportDecl, ExportDefaultExpr,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, ForHead, ForInStmt, Ident, IdentName,
    ImportDecl, ImportDefaultSpecifier, ImportNamedSpecifier, ImportSpecifier, Lit, MemberExpr,
    MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectPatProp,
    OptChainBase, Pat, Prop, PropName, PropOrSpread, ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt,
    Str, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::utils::{find_pat_ids, ExprFactory};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::{BindingId, BindingUseIndex};
use crate::utils::paren::strip_parens;

use super::decl_utils::{collect_decl_names, collect_pat_names, same_ident};
use super::rename_utils::{rename_bindings, BindingRename};
use super::RewriteLevel;

pub struct UnEsm {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnEsm {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
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
        // Phase -1: hoist require() calls out of complex expressions
        hoist_embedded_requires(module, self.unresolved_mark);
        // Phase 0: split compound `var s = exports.X = expr` →
        //          `var s = expr; exports.X = s;`
        split_compound_exports(module, self.unresolved_mark);
        rewrite_webpack_export_getters(module, self.unresolved_mark);
        lower_exported_cjs_requires(module, self.unresolved_mark);
        let all_declared_names = collect_all_declared_names(module);
        let binding_uses = BindingUseIndex::collect(module);
        let require_bindings =
            collect_stable_require_bindings(module, &binding_uses, self.unresolved_mark);

        let items = std::mem::take(&mut module.body);

        // Phase 1: classify
        let mut classified: Vec<Classified> = Vec::with_capacity(items.len());

        for item in items {
            classified.push(classify_item(item, self.unresolved_mark, &require_bindings));
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
                        new_body.extend(build_export_items(span, kind));
                    }
                }
                Classified::Keep(item) => {
                    new_body.push(item);
                }
            }
        }

        merge_decl_and_named_export(&mut new_body);
        inline_adjacent_default_export_aliases(&mut new_body);
        module.body = new_body;
    }
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
        if count_binding_refs(body, &alias_key, Some(index)) != 1 {
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

fn count_binding_refs(
    body: &[ModuleItem],
    key: &(Atom, SyntaxContext),
    skip_index: Option<usize>,
) -> usize {
    let mut counter = BindingRefCounter { key, count: 0 };
    for (index, item) in body.iter().enumerate() {
        if skip_index == Some(index) {
            continue;
        }
        item.visit_with(&mut counter);
    }
    counter.count
}

struct BindingRefCounter<'a> {
    key: &'a (Atom, SyntaxContext),
    count: usize,
}

impl Visit for BindingRefCounter<'_> {
    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == self.key.0 && id.ctxt == self.key.1 {
            self.count += 1;
        }
    }
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
                    if let Expr::Ident(ident) = *expr {
                        // Webpack5 getter `() => ident` is a live accessor.
                        // Emit `export { ident as default }` directly — this
                        // preserves live-binding semantics and avoids TDZ,
                        // bypassing CJS classification (which would snapshot).
                        deferred_default.push(ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(
                            NamedExport {
                                span: item_span,
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
                            },
                        )));
                    } else {
                        deferred_default.push(make_exports_assign_expr_item(
                            item_span,
                            (name, expr),
                            unresolved_mark,
                        ));
                    }
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
            // IIFE getters already reject `default` entries, so no TDZ risk.
            // Keep them in-place to preserve adjacency with their declarations
            // for merge_decl_and_named_export.
            new_body.extend(
                exports.into_iter().map(|export| {
                    make_exports_assign_expr_item(item_span, export, unresolved_mark)
                }),
            );
            continue;
        }

        if converted_getter_map && is_exports_default_compat_block(&item, unresolved_mark) {
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

fn expose_unused_iife_webpack_export_getters(module: &mut Module, unresolved_mark: Mark) {
    if module.body.len() != 1 {
        return;
    }
    let Some(expanded) =
        extract_unused_iife_webpack_export_getter_body(&module.body[0], unresolved_mark)
    else {
        return;
    };
    module.body = expanded;
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
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
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

fn block_contains_direct_webpack_export_getter(block: &BlockStmt, unresolved_mark: Mark) -> bool {
    block.stmts.iter().any(|stmt| {
        extract_direct_webpack_export_getters(&ModuleItem::Stmt(stmt.clone()), unresolved_mark)
            .is_some()
    })
}

fn block_has_top_level_return(block: &BlockStmt) -> bool {
    block
        .stmts
        .iter()
        .any(|stmt| matches!(stmt, Stmt::Return(_)))
}

fn arrow_params_used_in_block(arrow: &ArrowExpr, block: &BlockStmt) -> bool {
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

fn block_contains_arguments_ident(block: &BlockStmt) -> bool {
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
        if !is_valid_js_ident(export_name) {
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
    if exports.is_empty() || exports.iter().any(|(name, _)| name.as_ref() == "default") {
        return None;
    }
    Some(exports)
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
    let BlockStmtOrExpr::BlockStmt(block) = arrow.body.as_ref() else {
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
                BlockStmtOrExpr::BlockStmt(block) => extract_single_return_expr(block),
                BlockStmtOrExpr::Expr(expr) => Some(expr.clone()),
            }
        }
        _ => None,
    }
}

fn extract_single_return_expr(block: &BlockStmt) -> Option<Box<Expr>> {
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
    let Some(call) = expr_stmt_call(stmt) else {
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
    let Some(call) = expr_stmt_call(stmt) else {
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
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
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

fn expr_stmt_call(stmt: &Stmt) -> Option<&CallExpr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    Some(call)
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

fn build_export_items(span: Span, kind: CjsExportKind) -> Vec<ModuleItem> {
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
            })
        }
        _ => false,
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
}

fn binding_id(ident: &Ident) -> BindingId {
    (ident.sym.clone(), ident.ctxt)
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

/// Split compound `var s = exports.X = expr` into `var s = expr; exports.X = s;`
/// so the normal export classification can handle the extracted `exports.X = s` statement.
fn split_compound_exports(module: &mut Module, unresolved_mark: Mark) {
    let mut new_body = Vec::with_capacity(module.body.len());
    for item in std::mem::take(&mut module.body) {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var))) = item else {
            new_body.push(item);
            continue;
        };
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
        let candidate = Atom::from(format!("_{name}{index}"));
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
