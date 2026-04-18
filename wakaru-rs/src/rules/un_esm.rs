use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BindingIdent, CallExpr, Callee, Decl, ExportDecl, ExportDefaultExpr,
    ExportNamedSpecifier, ExportSpecifier, Expr, Ident, IdentName, ImportDecl,
    ImportDefaultSpecifier, ImportNamedSpecifier, ImportSpecifier, Lit, MemberExpr, MemberProp,
    Module, ModuleDecl, ModuleExportName, ModuleItem, NamedExport, ObjectPatProp, Pat,
    SimpleAssignTarget, Stmt, Str, UnaryOp, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::VisitMut;

use super::rename_utils::{rename_bindings, BindingRename};

pub struct UnEsm;

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
        // Phase 0: split compound `var s = exports.X = expr` →
        //          `var s = expr; exports.X = s;`
        split_compound_exports(module);

        let items = std::mem::take(&mut module.body);

        // Phase 1: classify
        let mut classified: Vec<Classified> = Vec::with_capacity(items.len());

        for item in items {
            classified.push(classify_item(item));
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
            if let Classified::CjsExport { kind } = c {
                if let CjsExportKind::Named {
                    name,
                    expr,
                    is_void: false,
                } = kind
                {
                    let is_ident = matches!(expr.as_ref(), Expr::Ident(_));
                    if !is_ident && local_names.contains(name) {
                        export_names.insert(name.clone());
                    }
                }
            }
        }

        // Rename conflicting locals before building exports. The export
        // expression can reference a conflicting module-level local, so apply
        // binding-id renames to both kept items and export expressions.
        if !export_names.is_empty() {
            let mut used_names = local_names.clone();
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
                        // drop
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

// ============================================================
// Classification helpers
// ============================================================

fn classify_item(item: ModuleItem) -> Classified {
    match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => Classified::ExistingImport(import),
        ModuleItem::Stmt(ref stmt) => {
            if let Some(kind) = try_classify_cjs_export(stmt) {
                return Classified::CjsExport { kind };
            }
            if let Some(kind) = try_classify_cjs_require(stmt) {
                return Classified::CjsRequire(kind);
            }
            Classified::Keep(item)
        }
        other => Classified::Keep(other),
    }
}

/// Split compound `var s = exports.X = expr` into `var s = expr; exports.X = s;`
/// so the normal export classification can handle the extracted `exports.X = s` statement.
fn split_compound_exports(module: &mut Module) {
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
            if let Some((export_name, real_init)) = try_extract_exports_assign(init) {
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
                            obj: Box::new(Expr::Ident(Ident::new(
                                "exports".into(),
                                DUMMY_SP,
                                binding.id.ctxt,
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
fn try_extract_exports_assign(expr: &Expr) -> Option<(Atom, Box<Expr>)> {
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
    if obj_id.sym.as_ref() != "exports" {
        return None;
    }
    let Some(prop_name) = is_ident_prop(&member.prop) else {
        return None;
    };
    Some((prop_name, assign.right.clone()))
}

/// Try to classify as a CJS export statement
fn try_classify_cjs_export(stmt: &Stmt) -> Option<CjsExportKind> {
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
    if is_module_exports_expr(&member.obj) {
        // module.exports.foo = expr
        if let Some(prop) = is_ident_prop(&member.prop) {
            if prop.as_ref() == "default" {
                // module.exports.default = module.exports → self-ref
                if is_module_exports_expr(&assign.right) {
                    return Some(CjsExportKind::SelfRef);
                }
                return Some(CjsExportKind::NamedDefault {
                    expr: assign.right.clone(),
                });
            }
            let is_void = is_void_or_undefined(&assign.right);
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
        if obj_id.sym.as_ref() == "module" {
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
        if obj_id.sym.as_ref() == "exports" {
            if let Some(prop) = is_ident_prop(&member.prop) {
                if prop.as_ref() == "default" {
                    return Some(CjsExportKind::NamedDefault {
                        expr: assign.right.clone(),
                    });
                }
                let is_void = is_void_or_undefined(&assign.right);
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
fn try_classify_cjs_require(stmt: &Stmt) -> Option<CjsRequireKind> {
    match stmt {
        // Bare require: require('foo');
        Stmt::Expr(expr_stmt) => {
            if let Expr::Call(call) = expr_stmt.expr.as_ref() {
                if let Some(source) = is_require_call(call) {
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
                        if let Some(source) = is_require_call(call) {
                            return Some(CjsRequireKind::Default { local, source });
                        }
                    }
                    // var foo = require('bar').baz or require('bar').default
                    if let Expr::Member(member) = init.as_ref() {
                        if let Expr::Call(call) = member.obj.as_ref() {
                            if let Some(source) = is_require_call(call) {
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
                        if let Some(source) = is_require_call(call) {
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
fn is_require_call(call: &CallExpr) -> Option<String> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };
    if id.sym.as_ref() != "require" {
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
fn is_void_or_undefined(expr: &Expr) -> bool {
    match expr {
        Expr::Unary(unary) if unary.op == UnaryOp::Void => true,
        Expr::Ident(id) if id.sym.as_ref() == "undefined" => true,
        _ => false,
    }
}

/// Check if expr is `module.exports`
fn is_module_exports_expr(expr: &Expr) -> bool {
    if let Expr::Member(MemberExpr { obj, prop, .. }) = expr {
        if let Expr::Ident(id) = obj.as_ref() {
            if id.sym.as_ref() == "module" {
                if let MemberProp::Ident(IdentName { sym, .. }) = prop {
                    return sym.as_ref() == "exports";
                }
            }
        }
    }
    false
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

fn wtf8_to_string(value: &swc_core::atoms::Wtf8Atom) -> String {
    value.as_str().unwrap_or("").to_string()
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
