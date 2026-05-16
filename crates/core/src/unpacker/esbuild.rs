use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, BindingIdent, BlockStmtOrExpr, CallExpr, Callee, ClassDecl, Decl, ExportDecl,
    ExportNamedSpecifier, ExportSpecifier, Expr, ExprStmt, FnDecl, ForInStmt, Ident, ImportDecl,
    ImportNamedSpecifier, ImportSpecifier, Module, ModuleDecl, ModuleExportName, ModuleItem,
    NamedExport, ObjectLit, Pat, PropName, Stmt, Str, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMutWith, VisitWith};

use crate::unpacker::{module_item_declared_binding_ids, BindingId, UnpackResult, UnpackedModule};

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "esbuild.js", cm.clone()).ok()?;
        detect_from_module(&module, cm)
    })
}

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
    let mut analysis_module = module.clone();
    analysis_module.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

    // Phase 1: find the lazy-helper variables (esbuild's __commonJS / __esm equivalents).
    let helper_syms = collect_helper_syms(module);

    // Phase 2: collect factory declarations — `var X = helper(factory_fn)`.
    let factories = if helper_syms.is_empty() {
        vec![]
    } else {
        collect_factories(module, &helper_syms)
    };

    let has_factories = factories.len() >= 5;

    // Try scope-hoisted detection on the full module body (needed for
    // scope-only bundles that have no factories at all).
    // Two+ boundaries is strong evidence on its own. A single boundary
    // needs corroboration: the namespace must appear in an ESM export
    // declaration (e.g. `export { math_exports as math }`), which is
    // always true for esbuild but not for coincidental helper code.
    let has_scope_hoisted = detect_export_helper(&analysis_module.body)
        .map(|(_, helper)| {
            let refs = build_item_binding_infos(&analysis_module.body);
            let boundaries = collect_scope_hoisted_boundaries(&analysis_module.body, &helper);
            match boundaries.len() {
                0 => false,
                1 => namespace_is_module_exported(
                    &analysis_module.body,
                    &refs,
                    &boundaries[0].ns_binding,
                ),
                _ => true,
            }
        })
        .unwrap_or(false);

    if !has_factories && !has_scope_hoisted {
        return None;
    }

    let factory_syms: HashSet<Atom> = factories.iter().map(|f| f.var_name.clone()).collect();

    // Phase 3: emit each factory as an individual module.
    // Dedup factory filenames with the same case-insensitive probe loop used
    // by the CLI, so that the names the scope-hoisted pass seeds from already
    // reflect any CLI-level renames (e.g. `a.js` + `A.js` → `a.js` + `A_2.js`).
    let mut modules: Vec<UnpackedModule> = Vec::new();
    let mut global_seen: HashSet<String> = HashSet::new();
    global_seen.insert("entry.js".to_string());

    for factory in factories {
        let filename = dedup_filename(&factory.filename, &mut global_seen);
        let code = emit_stmts(factory.body_stmts, filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: factory.var_name.to_string(),
            is_entry: false,
            code,
            filename,
        });
    }

    // Phase 4: everything that is not a helper decl or factory decl becomes the entry.
    let entry_items: Vec<ModuleItem> = module
        .body
        .iter()
        .filter(|item| !is_helper_or_factory_item(item, &helper_syms, &factory_syms))
        .cloned()
        .collect();
    let analysis_entry_items: Vec<ModuleItem> = analysis_module
        .body
        .iter()
        .filter(|item| !is_helper_or_factory_item(item, &helper_syms, &factory_syms))
        .cloned()
        .collect();

    // Phase 5: split scope-hoisted modules out of the entry items.
    let (scope_hoisted_modules, remaining_entry) = extract_scope_hoisted_modules(
        &analysis_entry_items,
        &entry_items,
        &mut global_seen,
        cm.clone(),
    );
    modules.extend(scope_hoisted_modules);

    if !remaining_entry.is_empty() {
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
        });
    }

    Some(UnpackResult { modules })
}

// ---------------------------------------------------------------------------
// Extracted factory info
// ---------------------------------------------------------------------------

struct Factory {
    /// The declared variable name (e.g. `BO7`).
    var_name: Atom,
    /// Derived filename: filepath string key when available, else `<var_name>.js`.
    filename: String,
    /// The statements inside the factory function body.
    body_stmts: Vec<Stmt>,
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

fn collect_factories(module: &Module, helper_syms: &HashSet<Atom>) -> Vec<Factory> {
    let mut factories = Vec::new();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            if let Some(factory) = try_extract_factory(decl, helper_syms) {
                factories.push(factory);
            }
        }
    }
    factories
}

fn try_extract_factory(decl: &VarDeclarator, helper_syms: &HashSet<Atom>) -> Option<Factory> {
    let Pat::Ident(var_ident) = &decl.name else {
        return None;
    };
    let init = decl.init.as_ref()?;
    let Expr::Call(call) = &**init else {
        return None;
    };

    // Callee must be one of the detected helpers.
    if !call_targets_helper(call, helper_syms) {
        return None;
    }

    if call.args.len() != 1 {
        return None;
    }

    let arg = &*call.args[0].expr;
    let var_name = var_ident.id.sym.clone();

    match arg {
        // Non-minified: __commonJS({ "src/foo.js"(exports, module) { … } })
        Expr::Object(obj) if obj.props.len() == 1 => {
            use swc_core::ecma::ast::{Prop, PropOrSpread};
            if let PropOrSpread::Prop(prop) = &obj.props[0] {
                if let Prop::Method(method) = &**prop {
                    let filename = prop_key_str(&method.key)
                        .map(sanitize_path)
                        .unwrap_or_else(|| format!("{var_name}.js"));
                    let body_stmts = method.function.body.as_ref()?.stmts.clone();
                    return Some(Factory {
                        var_name,
                        filename,
                        body_stmts,
                    });
                }
            }
            None
        }

        // Minified arrow: y(() => { … }) or y(() => expr)
        Expr::Arrow(arrow) => {
            let body_stmts = arrow_body_stmts(arrow);
            let filename = format!("{var_name}.js");
            Some(Factory {
                var_name,
                filename,
                body_stmts,
            })
        }

        // Minified function: m(function() { … })
        Expr::Fn(fn_expr) => {
            let body_stmts = fn_expr.function.body.as_ref()?.stmts.clone();
            let filename = format!("{var_name}.js");
            Some(Factory {
                var_name,
                filename,
                body_stmts,
            })
        }

        _ => None,
    }
}

fn call_targets_helper(call: &CallExpr, helper_syms: &HashSet<Atom>) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(ident) = &**callee else {
        return false;
    };
    helper_syms.contains(&ident.sym)
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
    // Strip leading `../` segments so the path doesn't escape the output directory.
    let s = s.trim_start_matches("../");
    if s.is_empty() {
        "module.js".to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Entry module filter
// ---------------------------------------------------------------------------

fn is_helper_or_factory_item(
    item: &ModuleItem,
    helper_syms: &HashSet<Atom>,
    factory_syms: &HashSet<Atom>,
) -> bool {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return false;
    };
    var.decls.iter().any(|d| {
        let Pat::Ident(bi) = &d.name else {
            return false;
        };
        let sym = &bi.id.sym;
        helper_syms.contains(sym) || factory_syms.contains(sym)
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
struct ScopeModuleMeta {
    body_indices: Vec<usize>,
    exported_atoms: HashSet<Atom>,
    declared_bindings: HashSet<BindingId>,
    referenced_bindings: HashSet<BindingId>,
    filename: String,
    id: String,
}

/// Extract scope-hoisted modules from entry items.
/// Returns (extracted_modules, remaining_entry_items).
///
/// After partitioning items into per-module groups, this function
/// synthesizes ES import/export statements so that cross-module
/// references (which the bundler resolved via direct bindings) are
/// represented as standard module edges.
///
/// `seen_lower` is the shared case-insensitive filename set, already
/// populated by factory modules.  Scope-hoisted filenames are probed
/// against it so they never collide with factories or each other.
fn extract_scope_hoisted_modules(
    analysis_items: &[ModuleItem],
    source_items: &[ModuleItem],
    seen_lower: &mut HashSet<String>,
    cm: Lrc<SourceMap>,
) -> (Vec<UnpackedModule>, Vec<ModuleItem>) {
    debug_assert_eq!(analysis_items.len(), source_items.len());

    // Step 1: find the __export helper binding.
    let Some((export_helper_index, export_helper)) = detect_export_helper(analysis_items) else {
        return (vec![], source_items.to_vec());
    };
    let item_infos = build_item_binding_infos(analysis_items);

    // Step 2: find all (namespace_decl_index, export_call_index, ns_atom) triples.
    let boundaries = collect_scope_hoisted_boundaries(analysis_items, &export_helper);
    if boundaries.is_empty() {
        return (vec![], source_items.to_vec());
    }

    // Step 3 (pass 1): partition items and collect per-module metadata.
    let mut metas: Vec<ScopeModuleMeta> = Vec::new();
    let mut consumed: HashSet<usize> = HashSet::new();

    consumed.insert(export_helper_index);

    // Track consumed namespace bindings so we can restore them for the entry.
    let mut consumed_ns: Vec<(usize, usize, &ScopeHoistedBoundary)> = Vec::new();

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
            )
        };

        let mut body_indices: Vec<usize> = Vec::new();
        let mut declared_bindings: HashSet<BindingId> = HashSet::new();
        let mut referenced_bindings: HashSet<BindingId> = HashSet::new();

        for (i, info) in item_infos.iter().enumerate().take(end).skip(start) {
            consumed.insert(i);
            if i == boundary.ns_decl_index || i == boundary.export_call_index {
                continue;
            }
            body_indices.push(i);
            declared_bindings.extend(info.declared.iter().cloned());
            referenced_bindings.extend(info.references.iter().cloned());
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
            body_indices,
            exported_atoms,
            declared_bindings,
            referenced_bindings,
            filename,
            id,
        });
    }

    // Build binding → module index map for all scope-hoisted modules.
    let mut binding_to_module: HashMap<BindingId, usize> = HashMap::new();
    for (mi, meta) in metas.iter().enumerate() {
        for binding in &meta.declared_bindings {
            binding_to_module.insert(binding.clone(), mi);
        }
    }

    // Collect remaining entry references early so they feed into the
    // effective-export expansion below.
    let remaining_indices: Vec<usize> = (0..source_items.len())
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
    }
    for ref_binding in &entry_referenced {
        if let Some(&source_mi) = binding_to_module.get(ref_binding) {
            effective_exports[source_mi].insert(ref_binding.0.clone());
        }
    }

    // Step 4 (pass 2): emit each module with synthesized imports/exports.
    let mut modules = Vec::new();

    for (mi, meta) in metas.iter().enumerate() {
        let mut module_items: Vec<ModuleItem> = Vec::new();
        let exports = &effective_exports[mi];

        // Synthesize imports from other scope-hoisted modules.
        let mut imports_by_source: HashMap<usize, Vec<Atom>> = HashMap::new();
        for ref_binding in &meta.referenced_bindings {
            if meta.declared_bindings.contains(ref_binding) {
                continue;
            }
            if let Some(&source_mi) = binding_to_module.get(ref_binding) {
                if source_mi != mi {
                    imports_by_source
                        .entry(source_mi)
                        .or_default()
                        .push(ref_binding.0.clone());
                }
            }
        }
        let mut import_sources: Vec<usize> = imports_by_source.keys().copied().collect();
        import_sources.sort();
        for source_mi in import_sources {
            let names = imports_by_source.get_mut(&source_mi).unwrap();
            names.sort();
            names.dedup();
            module_items.push(make_scope_import_stmt(names, &metas[source_mi].filename));
        }

        // Body items with export promotion for exported bindings.
        let mut remaining_exports = exports.clone();
        for &i in &meta.body_indices {
            let item = &source_items[i];
            if remaining_exports.is_empty() {
                module_items.push(item.clone());
                continue;
            }
            match try_promote_scope_export(item, &remaining_exports) {
                ScopeExportPromotion::Promoted(new_item, promoted) => {
                    module_items.push(new_item);
                    for name in &promoted {
                        remaining_exports.remove(name);
                    }
                }
                ScopeExportPromotion::None => {
                    module_items.push(item.clone());
                }
            }
        }
        if !remaining_exports.is_empty() {
            let mut names: Vec<Atom> = remaining_exports.into_iter().collect();
            names.sort();
            module_items.push(make_scope_export_stmt(&names));
        }

        let code = emit_items(module_items, meta.filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: meta.id.clone(),
            is_entry: false,
            code,
            filename: meta.filename.clone(),
        });
    }

    // Restore consumed namespace decls + __export calls whose namespace
    // binding is still referenced by the remaining entry (e.g. the entry
    // has `export { math_exports as math }` but `var math_exports = {}`
    // was consumed).  Re-inserting them keeps the namespace object alive;
    // importing the individual bindings ensures the __export getters
    // resolve correctly.
    let mut restored_items: Vec<ModuleItem> = Vec::new();
    let mut need_export_helper = false;
    for &(ns_idx, call_idx, boundary) in &consumed_ns {
        if !entry_referenced.contains(&boundary.ns_binding) {
            continue;
        }
        need_export_helper = true;
        restored_items.push(source_items[ns_idx].clone());
        restored_items.push(source_items[call_idx].clone());
    }
    if need_export_helper {
        restored_items.insert(0, source_items[export_helper_index].clone());
    }

    let mut entry_imports: HashMap<usize, Vec<Atom>> = HashMap::new();
    for ref_binding in &entry_referenced {
        if let Some(&source_mi) = binding_to_module.get(ref_binding) {
            entry_imports
                .entry(source_mi)
                .or_default()
                .push(ref_binding.0.clone());
        }
    }

    let mut remaining: Vec<ModuleItem> = Vec::new();
    if !entry_imports.is_empty() {
        let mut import_sources: Vec<usize> = entry_imports.keys().copied().collect();
        import_sources.sort();
        for source_mi in import_sources {
            let names = entry_imports.get_mut(&source_mi).unwrap();
            names.sort();
            names.dedup();
            remaining.push(make_scope_import_stmt(names, &metas[source_mi].filename));
        }
    }
    remaining.extend(restored_items);
    remaining.extend(remaining_indices.iter().map(|&i| source_items[i].clone()));

    (modules, remaining)
}

struct ScopeHoistedBoundary {
    ns_atom: Atom,
    ns_binding: BindingId,
    ns_decl_index: usize,
    export_call_index: usize,
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

        let exported_bindings = extract_export_bindings(&items[i + 1]);

        boundaries.push(ScopeHoistedBoundary {
            ns_atom: ns_binding.0.clone(),
            ns_binding,
            ns_decl_index: i,
            export_call_index: i + 1,
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
fn extract_export_bindings(item: &ModuleItem) -> HashSet<BindingId> {
    let mut bindings = HashSet::new();
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item else {
        return bindings;
    };
    let Expr::Call(call) = &**expr else {
        return bindings;
    };
    if call.args.len() != 2 {
        return bindings;
    }
    let Expr::Object(obj) = &*call.args[1].expr else {
        return bindings;
    };
    for prop in &obj.props {
        let swc_core::ecma::ast::PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        let swc_core::ecma::ast::Prop::KeyValue(kv) = &**prop else {
            continue;
        };
        // Value is `() => binding` — extract the binding ident from the arrow body.
        let Expr::Arrow(arrow) = &*kv.value else {
            continue;
        };
        if let BlockStmtOrExpr::Expr(body_expr) = &*arrow.body {
            if let Expr::Ident(id) = &**body_expr {
                bindings.insert((id.sym.clone(), id.ctxt));
            }
        }
    }
    bindings
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
    // referenced by the module code collected so far. This captures private
    // helpers that esbuild emits after the exported functions that call them.
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
        if !declared
            .iter()
            .any(|binding| items_reference_binding(&item_infos[from..end], binding))
        {
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

#[derive(Default)]
struct ItemBindingInfo {
    declared: HashSet<BindingId>,
    references: HashSet<BindingId>,
}

fn build_item_binding_infos(items: &[ModuleItem]) -> Vec<ItemBindingInfo> {
    let top_level_bindings: HashSet<BindingId> = items
        .iter()
        .flat_map(module_item_declared_binding_ids)
        .collect();

    items
        .iter()
        .map(|item| {
            let mut collector = TopLevelRefCollector {
                top_level_bindings: &top_level_bindings,
                references: HashSet::new(),
            };
            item.visit_with(&mut collector);
            ItemBindingInfo {
                declared: module_item_declared_binding_ids(item).into_iter().collect(),
                references: collector.references,
            }
        })
        .collect()
}

struct TopLevelRefCollector<'a> {
    top_level_bindings: &'a HashSet<BindingId>,
    references: HashSet<BindingId>,
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

// ---------------------------------------------------------------------------
// Import / export synthesis for scope-hoisted modules
// ---------------------------------------------------------------------------

fn make_scope_import_stmt(names: &[Atom], from: &str) -> ModuleItem {
    let specifiers = names
        .iter()
        .map(|name| {
            ImportSpecifier::Named(ImportNamedSpecifier {
                span: Default::default(),
                local: Ident::new(name.clone(), Default::default(), Default::default()),
                imported: None,
                is_type_only: false,
            })
        })
        .collect();
    ModuleItem::ModuleDecl(ModuleDecl::Import(ImportDecl {
        span: Default::default(),
        specifiers,
        src: Box::new(Str {
            span: Default::default(),
            value: format!("./{from}").into(),
            raw: None,
        }),
        type_only: false,
        with: None,
        phase: Default::default(),
    }))
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

enum ScopeExportPromotion {
    Promoted(ModuleItem, Vec<Atom>),
    None,
}

fn try_promote_scope_export(item: &ModuleItem, exported: &HashSet<Atom>) -> ScopeExportPromotion {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
            if exported.contains(&fn_decl.ident.sym) =>
        {
            let names = vec![fn_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Fn(fn_decl.clone()),
            }));
            ScopeExportPromotion::Promoted(new_item, names)
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(class_decl)))
            if exported.contains(&class_decl.ident.sym) =>
        {
            let names = vec![class_decl.ident.sym.clone()];
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Class(class_decl.clone()),
            }));
            ScopeExportPromotion::Promoted(new_item, names)
        }
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) => {
            let all_exported = var_decl
                .decls
                .iter()
                .all(|d| matches!(&d.name, Pat::Ident(bi) if exported.contains(&bi.id.sym)));
            if !all_exported {
                return ScopeExportPromotion::None;
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
                return ScopeExportPromotion::None;
            }
            let new_item = ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                span: Default::default(),
                decl: Decl::Var(var_decl.clone()),
            }));
            ScopeExportPromotion::Promoted(new_item, names)
        }
        _ => ScopeExportPromotion::None,
    }
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

/// Emit factory body statements as raw JavaScript — no rules, no resolver.
/// The driver's `decompile()` will run the full pipeline on the emitted text.
fn emit_stmts(stmts: Vec<Stmt>, filename: String, cm: Lrc<SourceMap>) -> String {
    let module = Module {
        span: Default::default(),
        body: stmts.into_iter().map(ModuleItem::Stmt).collect(),
        shebang: None,
    };
    emit_module(module, filename, cm)
}

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
