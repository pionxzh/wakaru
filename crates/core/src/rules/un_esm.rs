use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent, BlockStmt,
    BlockStmtOrExpr, CallExpr, Callee, CondExpr, Decl, ExportDecl, ExportDefaultExpr,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, ForHead, ForInStmt, Ident, IdentName,
    ImportDecl, ImportDefaultSpecifier, ImportNamedSpecifier, ImportSpecifier, Lit, MemberExpr,
    MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectPatProp, Pat,
    Prop, PropName, PropOrSpread, ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt, Str, UnaryOp,
    VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitWith};

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
    /// module.exports.default = module.exports pattern → remove
    SelfRef,
}

/// Classification of a module item
enum Classified {
    ExistingImport(ImportDecl),
    CjsRequire(CjsRequireKind),
    CjsExport { kind: CjsExportKind },
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
        let all_declared_names = collect_all_declared_names(module);

        let items = std::mem::take(&mut module.body);

        // Phase 1: classify
        let mut classified: Vec<Classified> = Vec::with_capacity(items.len());

        for item in items {
            classified.push(classify_item(item, self.unresolved_mark));
        }

        // Phase 2: export dedup
        struct ExportEntry {
            classified_idx: usize,
            name: Option<Atom>, // None = default
            is_void: bool,
        }

        let mut export_entries: Vec<ExportEntry> = Vec::new();
        for (idx, c) in classified.iter().enumerate() {
            if let Classified::CjsExport { kind } = c {
                let (name, is_void) = match kind {
                    CjsExportKind::ModuleExportsDefault { .. } => (None, false),
                    CjsExportKind::NamedDefault { .. } => (None, false),
                    CjsExportKind::Named { name, is_void, .. } => (Some(name.clone()), *is_void),
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

        // Phase 3: collect imports — build source_map keyed by String
        let mut source_order: Vec<String> = Vec::new();
        let mut source_map: HashMap<String, SourceEntry> = HashMap::new();

        // First pass: mark which sources have CJS requires
        let mut cjs_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
        for c in classified.iter() {
            let src = match c {
                Classified::CjsRequire(CjsRequireKind::Bare { source }) => source.clone(),
                Classified::CjsRequire(CjsRequireKind::Default { source, .. }) => source.clone(),
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
                collect_decl_names_into(decl, &mut local_names);
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
                        Classified::CjsExport { kind } => rename_export_kind(kind, &renames),
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
                Classified::CjsExport { kind } => {
                    if drop_set.contains(&idx) {
                        new_body.extend(build_dropped_export_side_effect_items(kind));
                    } else {
                        new_body.extend(build_export_items(kind));
                    }
                }
                Classified::Keep(item) => {
                    new_body.push(item);
                }
            }
        }

        module.body = new_body;
    }
}

fn get_or_insert<'a>(
    order: &mut Vec<String>,
    map: &'a mut HashMap<String, SourceEntry>,
    src: String,
) -> &'a mut SourceEntry {
    if !map.contains_key(&src) {
        order.push(src.clone());
        map.insert(src.clone(), SourceEntry::default());
    }
    map.get_mut(&src).unwrap()
}

fn rewrite_webpack_export_getters(module: &mut Module, unresolved_mark: Mark) {
    let mut converted_getter_map = false;
    let mut new_body = Vec::with_capacity(module.body.len());

    for item in std::mem::take(&mut module.body) {
        if let Some(exports) = extract_direct_webpack_export_getters(&item, unresolved_mark) {
            new_body.extend(
                exports
                    .into_iter()
                    .map(|export| make_exports_assign_item(export, unresolved_mark)),
            );
            continue;
        }

        if let Some(exports) = extract_webpack_export_getter_iife(&item, unresolved_mark) {
            converted_getter_map = true;
            new_body.extend(
                exports
                    .into_iter()
                    .map(|export| make_exports_assign_item(export, unresolved_mark)),
            );
            continue;
        }

        if converted_getter_map && is_exports_default_compat_block(&item, unresolved_mark) {
            continue;
        }

        new_body.push(item);
    }

    module.body = new_body;
}

fn extract_direct_webpack_export_getters(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<(Atom, Ident)>> {
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
        let ident = extract_getter_expr_return_ident(call.args[2].expr.as_ref())?;
        return Some(vec![(export_name.into(), ident)]);
    }

    None
}

fn extract_webpack_export_getter_iife(
    item: &ModuleItem,
    unresolved_mark: Mark,
) -> Option<Vec<(Atom, Ident)>> {
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
    let Expr::Arrow(arrow) = strip_expr_parens(callee_expr.as_ref()) else {
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
) -> Option<Vec<(Atom, Ident)>> {
    let mut exports = Vec::with_capacity(object.props.len());
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let (name, ident) = match prop.as_ref() {
            Prop::Method(method) => {
                let name = prop_name_as_atom(&method.key)?;
                if !method.function.params.is_empty()
                    || method.function.is_async
                    || method.function.is_generator
                {
                    return None;
                }
                let ident = extract_single_return_ident(method.function.body.as_ref()?)?;
                (name, ident)
            }
            Prop::KeyValue(entry) => {
                let name = prop_name_as_atom(&entry.key)?;
                let ident = extract_getter_expr_return_ident(entry.value.as_ref())?;
                (name, ident)
            }
            _ => return None,
        };
        exports.push((name, ident));
    }
    Some(exports)
}

fn extract_getter_expr_return_ident(expr: &Expr) -> Option<Ident> {
    match expr {
        Expr::Fn(fn_expr) => {
            if fn_expr.ident.is_some()
                || !fn_expr.function.params.is_empty()
                || fn_expr.function.is_async
                || fn_expr.function.is_generator
            {
                return None;
            }
            extract_single_return_ident(fn_expr.function.body.as_ref()?)
        }
        Expr::Arrow(arrow) => {
            if !arrow.params.is_empty() || arrow.is_async || arrow.is_generator {
                return None;
            }
            match arrow.body.as_ref() {
                BlockStmtOrExpr::BlockStmt(block) => extract_single_return_ident(block),
                BlockStmtOrExpr::Expr(expr) => {
                    if let Expr::Ident(id) = expr.as_ref() {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
            }
        }
        _ => None,
    }
}

fn extract_single_return_ident(block: &BlockStmt) -> Option<Ident> {
    if block.stmts.len() != 1 {
        return None;
    }
    let Stmt::Return(ReturnStmt { arg: Some(arg), .. }) = &block.stmts[0] else {
        return None;
    };
    let Expr::Ident(id) = arg.as_ref() else {
        return None;
    };
    Some(id.clone())
}

fn make_exports_assign_item((name, ident): (Atom, Ident), unresolved_mark: Mark) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
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
            right: Box::new(Expr::Ident(ident)),
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
    let Expr::Bin(bin) = strip_expr_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::LogicalAnd
        && is_exports_default_type_guard(bin.left.as_ref(), unresolved_mark)
        && is_exports_default_esmodule_undefined(bin.right.as_ref(), unresolved_mark)
}

fn is_exports_default_type_guard(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_expr_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::LogicalOr {
        return false;
    }
    let Expr::Bin(object_and_not_null) = strip_expr_parens(bin.right.as_ref()) else {
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
    let Expr::Bin(bin) = strip_expr_parens(expr) else {
        return false;
    };
    if bin.op != BinaryOp::EqEqEq {
        return false;
    }
    matches!(strip_expr_parens(bin.left.as_ref()), Expr::Unary(unary)
        if unary.op == UnaryOp::TypeOf && is_exports_default_expr(unary.arg.as_ref(), unresolved_mark))
        && matches!(strip_expr_parens(bin.right.as_ref()), Expr::Lit(Lit::Str(s))
            if s.value.as_str() == Some(expected))
}

fn is_exports_default_not_null(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_expr_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::NotEqEq
        && is_exports_default_expr(bin.left.as_ref(), unresolved_mark)
        && matches!(
            strip_expr_parens(bin.right.as_ref()),
            Expr::Lit(Lit::Null(_))
        )
}

fn is_exports_default_esmodule_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Bin(bin) = strip_expr_parens(expr) else {
        return false;
    };
    bin.op == BinaryOp::EqEqEq
        && is_exports_default_esmodule_expr(bin.left.as_ref(), unresolved_mark)
        && matches!(strip_expr_parens(bin.right.as_ref()), Expr::Ident(id) if is_undefined_ident(id, unresolved_mark))
}

fn is_exports_default_esmodule_expr(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(member) = strip_expr_parens(expr) else {
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
    let Expr::Member(member) = strip_expr_parens(expr) else {
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
    let Expr::Member(member) = strip_expr_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, object, unresolved_mark))
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == property)
}

fn is_member_expr(expr: &Expr, object: &str, property: &str) -> bool {
    let Expr::Member(member) = strip_expr_parens(expr) else {
        return false;
    };
    matches!(member.obj.as_ref(), Expr::Ident(id) if id.sym.as_ref() == object)
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == property)
}

fn is_map_lookup(expr: &Expr, map_param: &Ident, loop_ident: &Ident) -> bool {
    let Expr::Member(member) = strip_expr_parens(expr) else {
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

fn strip_expr_parens(mut expr: &Expr) -> &Expr {
    while let Expr::Paren(paren) = expr {
        expr = paren.expr.as_ref();
    }
    expr
}

fn same_ident(a: &Ident, b: &Ident) -> bool {
    a.sym == b.sym && a.ctxt == b.ctxt
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

fn build_export_items(kind: CjsExportKind) -> Vec<ModuleItem> {
    match kind {
        CjsExportKind::ModuleExportsDefault { expr } => vec![ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultExpr(ExportDefaultExpr {
                span: DUMMY_SP,
                expr,
            }),
        )],
        CjsExportKind::NamedDefault { expr } => vec![ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultExpr(ExportDefaultExpr {
                span: DUMMY_SP,
                expr,
            }),
        )],
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
                            span: DUMMY_SP,
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
                            span: DUMMY_SP,
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
                    span: DUMMY_SP,
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

fn build_dropped_export_side_effect_items(kind: CjsExportKind) -> Vec<ModuleItem> {
    let expr = match kind {
        CjsExportKind::ModuleExportsDefault { expr }
        | CjsExportKind::NamedDefault { expr }
        | CjsExportKind::Named {
            expr,
            is_void: false,
            ..
        } => expr,
        CjsExportKind::Named { is_void: true, .. } | CjsExportKind::SelfRef => return vec![],
    };

    vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: DUMMY_SP,
        expr,
    }))]
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
fn hoist_embedded_requires(module: &mut Module, unresolved_mark: Mark) {
    let mut new_body = Vec::with_capacity(module.body.len());
    let mut used_names = collect_all_declared_names(module);

    for item in std::mem::take(&mut module.body) {
        match &item {
            // Pattern 1: export default (seq_expr with require calls)
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(export_default)) => {
                // Unwrap parens
                let expr = strip_parens_ref(&export_default.expr);
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
                        if let Expr::Call(inner_call) = strip_parens_ref(callee) {
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
                                    callee: Callee::Expr(Box::new(Expr::Ident(local))),
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
        let expr_ref = strip_parens_ref(expr);

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
                    let right = strip_parens_ref(&assign.right);
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
    let expr = strip_parens_ref(expr);

    // Must be: <test> ? <cons> : <alt>
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = expr
    else {
        return None;
    };

    // test must be: (i = require("...")) && i.__esModule
    // or: i && i.__esModule (where i was assigned in an outer sequence)
    let test = strip_parens_ref(test);
    let Expr::Bin(bin) = test else {
        return None;
    };
    if bin.op != BinaryOp::LogicalAnd {
        return None;
    }

    // Right side must be: X.__esModule
    let right = strip_parens_ref(&bin.right);
    let Expr::Member(member) = right else {
        return None;
    };
    let Expr::Ident(member_obj) = strip_parens_ref(&member.obj) else {
        return None;
    };
    let MemberProp::Ident(IdentName { sym, .. }) = &member.prop else {
        return None;
    };
    if sym.as_ref() != "__esModule" {
        return None;
    }

    // Left side of && must contain the require assignment
    let left = strip_parens_ref(&bin.left);

    // Pattern: (i = require("..."))
    if let Expr::Assign(assign) = left {
        if assign.op == AssignOp::Assign {
            if let Some(target) = simple_assign_target_ident(&assign.left) {
                let right_inner = strip_parens_ref(&assign.right);
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

fn strip_parens_ref(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_ref(&paren.expr),
        _ => expr,
    }
}

fn is_same_ident_ref(expr: &Expr, ident: &Ident) -> bool {
    let expr = strip_parens_ref(expr);
    if let Expr::Ident(id) = expr {
        id.sym == ident.sym && id.ctxt == ident.ctxt
    } else {
        false
    }
}

fn matches_default_object_for_ident(expr: &Expr, ident: &Ident) -> bool {
    let Expr::Object(obj) = strip_parens_ref(expr) else {
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

fn classify_item(item: ModuleItem, unresolved_mark: Mark) -> Classified {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Classified::ExistingImport(import),
        ModuleItem::Stmt(ref stmt) => {
            if let Some(kind) = try_classify_cjs_export(stmt, unresolved_mark) {
                return Classified::CjsExport { kind };
            }
            if let Some(kind) = try_classify_cjs_require(stmt, unresolved_mark) {
                return Classified::CjsRequire(kind);
            }
            Classified::Keep(item)
        }
        other => Classified::Keep(other),
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
                    span: DUMMY_SP,
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
fn try_classify_cjs_export(stmt: &Stmt, unresolved_mark: Mark) -> Option<CjsExportKind> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
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

    // Check if obj is `module.exports` (the member object itself is `module.exports`)
    if is_module_exports_expr(&member.obj, unresolved_mark) {
        // module.exports.foo = expr
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

    // Check if obj is `exports` identifier
    if let Expr::Ident(obj_id) = member.obj.as_ref() {
        if is_unresolved_ident(obj_id, "exports", unresolved_mark) {
            if let Some(prop) = is_ident_prop(&member.prop) {
                if prop.as_ref() == "default" {
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
        }
    }

    None
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
            let decl = &var.decls[0];
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
                                            swc_core::ecma::ast::PropName::Ident(i) => {
                                                i.sym.clone()
                                            }
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

/// Collect binding names introduced by a declaration.
fn collect_decl_names_into(decl: &Decl, names: &mut HashSet<Atom>) {
    match decl {
        Decl::Var(var) => {
            for d in &var.decls {
                collect_pat_names(&d.name, names);
            }
        }
        Decl::Fn(f) => {
            names.insert(f.ident.sym.clone());
        }
        Decl::Class(c) => {
            names.insert(c.ident.sym.clone());
        }
        _ => {}
    }
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
        CjsExportKind::SelfRef => {}
    }
}

fn collect_pat_names(pat: &Pat, names: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(id) => {
            names.insert(id.id.sym.clone());
        }
        Pat::Array(arr) => {
            for p in arr.elems.iter().flatten() {
                collect_pat_names(p, names);
            }
        }
        Pat::Object(obj) => {
            for prop in &obj.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_names(&kv.value, names),
                    ObjectPatProp::Assign(a) => {
                        names.insert(a.key.id.sym.clone());
                    }
                    ObjectPatProp::Rest(r) => collect_pat_names(&r.arg, names),
                }
            }
        }
        Pat::Assign(a) => collect_pat_names(&a.left, names),
        Pat::Rest(r) => collect_pat_names(&r.arg, names),
        _ => {}
    }
}
