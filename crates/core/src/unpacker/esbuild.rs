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

    // Phase 2: collect factory declarations — `var X = helper(factory_fn)`.
    let factories = if helper_syms.is_empty() {
        vec![]
    } else {
        let span = tracing::info_span!("esbuild: collect factories");
        let _enter = span.enter();
        collect_factories(module, &analysis_module, &helper_syms)
    };

    let has_factories = factories.len() >= 5;

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
        .flat_map(module_item_declared_binding_ids)
        .collect();

    struct PendingFactory {
        var_name: Atom,
        filename: String,
        body_stmts: Vec<Stmt>,
        referenced_bindings: HashSet<BindingId>,
        write_bindings: HashSet<BindingId>,
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
            var_name: factory.var_name,
            filename,
            body_stmts: factory.body_stmts,
            referenced_bindings,
            write_bindings,
        });
    }

    // Aggregate all factory-referenced bindings for scope-hoisted export expansion.
    let all_factory_referenced: HashSet<BindingId> = pending_factories
        .iter()
        .flat_map(|f| f.referenced_bindings.iter().cloned())
        .collect();

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
    // Pass factory-referenced bindings so the extraction can expand exports
    // and return binding→module mapping for factory import synthesis.
    let (scope_hoisted_modules, remaining_entry, binding_to_filename, module_already_imports) = {
        let span = tracing::info_span!("esbuild: extract scope-hoisted modules");
        let _enter = span.enter();
        extract_scope_hoisted_modules(
            &analysis_entry_items,
            entry_items,
            &mut global_seen,
            cm.clone(),
            &all_factory_referenced,
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
        stmts: Vec<Stmt>,
        referenced_bindings: HashSet<BindingId>,
        write_bindings: HashSet<BindingId>,
    }

    let mut merged_factories: HashMap<String, Vec<MergedFactory>> = HashMap::new();
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

        if let (true, Some(fname)) = (is_single_target, target_filename) {
            merged_factories
                .entry(fname)
                .or_default()
                .push(MergedFactory {
                    stmts: factory.body_stmts,
                    referenced_bindings: factory.referenced_bindings,
                    write_bindings: factory.write_bindings,
                });
        } else {
            standalone_factories.push(factory);
        }
    }

    // Append merged factory bodies to their target modules, synthesizing
    // imports for any cross-module reads the factory body needs.
    if !merged_factories.is_empty() {
        for module in &mut modules {
            let Some(factories) = merged_factories.remove(&module.filename) else {
                continue;
            };
            let mut extra_imports: HashMap<String, Vec<Atom>> = HashMap::new();
            let mut all_stmts: Vec<Stmt> = Vec::new();

            let already_imported = module_already_imports
                .get(&module.filename)
                .cloned()
                .unwrap_or_default();

            for mf in factories {
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
                    }
                }
                all_stmts.extend(mf.stmts);
            }

            let mut import_items: Vec<ModuleItem> = Vec::new();
            let mut source_filenames: Vec<String> = extra_imports.keys().cloned().collect();
            source_filenames.sort();
            for source_filename in source_filenames {
                let names = extra_imports.get_mut(&source_filename).unwrap();
                names.sort();
                names.dedup();
                let rel_path = relative_import_path(&module.filename, &source_filename);
                import_items.push(make_scope_import_stmt(names, &rel_path));
            }

            let body_items: Vec<ModuleItem> = import_items
                .into_iter()
                .chain(all_stmts.into_iter().map(ModuleItem::Stmt))
                .collect();
            let extra_code = emit_items(body_items, module.filename.clone(), cm.clone());
            module.code.push('\n');
            module.code.push_str(&extra_code);
        }
    }

    for factory in standalone_factories {
        let mut import_items: Vec<ModuleItem> = Vec::new();

        if !binding_to_filename.is_empty() {
            // Group factory's referenced bindings by source module filename.
            let mut imports_by_source: HashMap<String, Vec<Atom>> = HashMap::new();
            for ref_binding in &factory.referenced_bindings {
                // Don't import bindings that this factory writes to.
                if factory.write_bindings.contains(ref_binding) {
                    continue;
                }
                if let Some(source_filename) = binding_to_filename.get(ref_binding) {
                    imports_by_source
                        .entry(source_filename.clone())
                        .or_default()
                        .push(ref_binding.0.clone());
                }
            }
            let mut source_filenames: Vec<String> = imports_by_source.keys().cloned().collect();
            source_filenames.sort();
            for source_filename in source_filenames {
                let names = imports_by_source.get_mut(&source_filename).unwrap();
                names.sort();
                names.dedup();
                let rel_path = relative_import_path(&factory.filename, &source_filename);
                import_items.push(make_scope_import_stmt(names, &rel_path));
            }
        }

        let body_items: Vec<ModuleItem> = import_items
            .into_iter()
            .chain(factory.body_stmts.into_iter().map(ModuleItem::Stmt))
            .collect();
        let code = emit_items(body_items, factory.filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: factory.var_name.to_string(),
            is_entry: false,
            code,
            filename: factory.filename,
        });
    }

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
    /// The statements inside the factory function body (unresolved — for emission).
    body_stmts: Vec<Stmt>,
    /// The statements inside the factory function body (resolved — for reference collection).
    analysis_body_stmts: Vec<Stmt>,
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

fn collect_factories(
    module: &Module,
    analysis_module: &Module,
    helper_syms: &HashSet<Atom>,
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
            if let Some(factory) = try_extract_factory(decl, analysis_decl, helper_syms) {
                factories.push(factory);
            }
        }
    }
    factories
}

fn try_extract_factory(
    decl: &VarDeclarator,
    analysis_decl: &VarDeclarator,
    helper_syms: &HashSet<Atom>,
) -> Option<Factory> {
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
                        .unwrap_or_else(|| format!("{var_name}.js"));
                    let body_stmts = method.function.body.as_ref()?.stmts.clone();
                    let analysis_body_stmts = extract_analysis_body_stmts_obj(analysis_arg)
                        .unwrap_or_else(|| body_stmts.clone());
                    return Some(Factory {
                        var_name,
                        filename,
                        body_stmts,
                        analysis_body_stmts,
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
            let filename = format!("{var_name}.js");
            Some(Factory {
                var_name,
                filename,
                body_stmts,
                analysis_body_stmts,
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
            let filename = format!("{var_name}.js");
            Some(Factory {
                var_name,
                filename,
                body_stmts,
                analysis_body_stmts,
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
    crate::unpacker::sanitize_relative_path(s, "module.js")
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
    factory_referenced: &HashSet<BindingId>,
) -> (
    Vec<UnpackedModule>,
    Vec<ModuleItem>,
    HashMap<BindingId, String>,
    HashMap<String, HashSet<BindingId>>,
) {
    debug_assert_eq!(analysis_items.len(), source_items.len());

    // Step 1: find the __export helper binding.
    let Some((export_helper_index, export_helper)) = detect_export_helper(analysis_items) else {
        return (vec![], source_items, HashMap::new(), HashMap::new());
    };
    let item_infos = build_item_binding_infos(analysis_items);

    // Step 2: find all (namespace_decl_index, export_call_index, ns_atom) triples.
    let boundaries = collect_scope_hoisted_boundaries(analysis_items, &export_helper);
    if boundaries.is_empty() {
        return (vec![], source_items, HashMap::new(), HashMap::new());
    }

    // Convert to Option<ModuleItem> so items can be moved out by index.
    let mut source_slots: Vec<Option<ModuleItem>> = source_items.into_iter().map(Some).collect();

    // Step 3 (pass 1): partition items and collect per-module metadata.
    let mut metas: Vec<ScopeModuleMeta> = Vec::new();
    let mut consumed: HashSet<usize> = HashSet::new();

    consumed.insert(export_helper_index);

    // Track consumed namespace bindings so we can restore them for the entry.
    let mut consumed_ns: Vec<(usize, usize, &ScopeHoistedBoundary)> = Vec::new();

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

    // Build binding→filename map so callers can synthesize imports in factory modules.
    let mut binding_to_filename: HashMap<BindingId, String> = binding_to_module
        .iter()
        .map(|(binding, &mi)| (binding.clone(), metas[mi].filename.clone()))
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
            let item = source_slots[i].take().expect("body item already consumed");
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

        let code = emit_items(module_items, meta.filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: meta.id.clone(),
            is_entry: false,
            code,
            filename: meta.filename.clone(),
        });
    }

    // Track which external bindings each scope-hoisted module already imports
    // (used later to avoid duplicate imports when merging init factories).
    let mut module_already_imports: HashMap<String, HashSet<BindingId>> = HashMap::new();
    for meta in &metas {
        let imported: HashSet<BindingId> = meta
            .referenced_bindings
            .iter()
            .filter(|b| !meta.declared_bindings.contains(b))
            .filter(|b| binding_to_module.contains_key(b))
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
    let mut need_export_helper = false;
    let mut factory_ns_exports: Vec<Atom> = Vec::new();
    for &(ns_idx, call_idx, boundary) in &consumed_ns {
        let entry_needs = entry_referenced.contains(&boundary.ns_binding);
        let factory_needs = factory_referenced.contains(&boundary.ns_binding);
        if !entry_needs && !factory_needs {
            continue;
        }
        need_export_helper = true;
        restored_items.push(
            source_slots[ns_idx]
                .take()
                .expect("ns_decl already consumed"),
        );
        restored_items.push(
            source_slots[call_idx]
                .take()
                .expect("export_call already consumed"),
        );
        // If a factory references this namespace but the entry doesn't
        // already export it via an ESM export declaration, synthesize one
        // so the factory's `import { ns_a } from "./entry.js"` resolves.
        if factory_needs && !entry_already_exports.contains(&boundary.ns_binding.0) {
            factory_ns_exports.push(boundary.ns_binding.0.clone());
        }
    }
    if need_export_helper {
        restored_items.insert(
            0,
            source_slots[export_helper_index]
                .take()
                .expect("export_helper already consumed"),
        );
    }
    if !factory_ns_exports.is_empty() {
        factory_ns_exports.sort();
        restored_items.push(make_scope_export_stmt(&factory_ns_exports));
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
    remaining.extend(remaining_indices.iter().map(|&i| {
        source_slots[i]
            .take()
            .expect("remaining item already consumed")
    }));

    (
        modules,
        remaining,
        binding_to_filename,
        module_already_imports,
    )
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

    let mut collector = WriteCollector {
        top_level_bindings,
        writes: out,
    };
    stmt.visit_with(&mut collector);
}

// ---------------------------------------------------------------------------
// Import / export synthesis for scope-hoisted modules
// ---------------------------------------------------------------------------

/// Compute a relative import specifier from `importer` to `target`.
/// Both are flat output filenames (e.g. `src/consumer.js`, `ns_a.js`).
/// Returns a string suitable for an ES import source (e.g. `./ns_a.js`,
/// `../ns_a.js`).
fn relative_import_path(importer: &str, target: &str) -> String {
    use std::path::Path;
    let importer_dir = Path::new(importer).parent().unwrap_or(Path::new(""));
    let rel = pathdiff_relative(importer_dir, Path::new(target));
    // Ensure the path starts with ./ or ../
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.starts_with("./") || s.starts_with("../") {
        s
    } else {
        format!("./{s}")
    }
}

/// Minimal relative path computation: `target` relative to `base` directory.
fn pathdiff_relative(base: &std::path::Path, target: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};

    let base_components: Vec<Component> = base.components().collect();
    let target_components: Vec<Component> = target.components().collect();

    let common = base_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut result = PathBuf::new();
    for _ in common..base_components.len() {
        result.push("..");
    }
    for component in &target_components[common..] {
        result.push(component);
    }
    result
}

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
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(ref var_decl)))
            if var_decl
                .decls
                .iter()
                .all(|d| matches!(&d.name, Pat::Ident(bi) if exported.contains(&bi.id.sym))) =>
        {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var_decl))) = item else {
                unreachable!()
            };
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
