use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, ExprStmt, ForInStmt, Module,
    ModuleItem, ObjectLit, Pat, Stmt, Str, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use crate::unpacker::{UnpackResult, UnpackedModule};

pub fn detect_and_extract(source: &str) -> Option<UnpackResult> {
    GLOBALS.set(&Default::default(), || {
        let cm: Lrc<SourceMap> = Default::default();
        let module = super::parse_es_module(source, "esbuild.js", cm.clone()).ok()?;
        detect_from_module(&module, cm)
    })
}

pub(super) fn detect_from_module(module: &Module, cm: Lrc<SourceMap>) -> Option<UnpackResult> {
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
    let has_scope_hoisted = detect_export_helper(&module.body)
        .map(|helper| {
            let boundaries = collect_scope_hoisted_boundaries(&module.body, &helper);
            match boundaries.len() {
                0 => false,
                1 => namespace_is_module_exported(&module.body, &boundaries[0].ns_atom),
                _ => true,
            }
        })
        .unwrap_or(false);

    if !has_factories && !has_scope_hoisted {
        return None;
    }

    let factory_syms: HashSet<Atom> = factories.iter().map(|f| f.var_name.clone()).collect();

    // Phase 3: emit each factory as an individual module.
    let mut modules: Vec<UnpackedModule> = Vec::new();

    for factory in factories {
        let code = emit_stmts(factory.body_stmts, factory.filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: factory.var_name.to_string(),
            is_entry: false,
            code,
            filename: factory.filename,
        });
    }

    // Phase 4: everything that is not a helper decl or factory decl becomes the entry.
    let entry_items: Vec<ModuleItem> = module
        .body
        .iter()
        .filter(|item| !is_helper_or_factory_item(item, &helper_syms, &factory_syms))
        .cloned()
        .collect();

    // Phase 5: split scope-hoisted modules out of the entry items.
    let (scope_hoisted_modules, remaining_entry) =
        extract_scope_hoisted_modules(&entry_items, cm.clone());
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

/// Extract scope-hoisted modules from entry items.
/// Returns (extracted_modules, remaining_entry_items).
fn extract_scope_hoisted_modules(
    items: &[ModuleItem],
    cm: Lrc<SourceMap>,
) -> (Vec<UnpackedModule>, Vec<ModuleItem>) {
    // Step 1: find the __export helper binding.
    let Some(export_helper) = detect_export_helper(items) else {
        return (vec![], items.to_vec());
    };

    // Step 2: find all (namespace_decl_index, export_call_index, ns_atom) triples.
    let boundaries = collect_scope_hoisted_boundaries(items, &export_helper);
    if boundaries.is_empty() {
        return (vec![], items.to_vec());
    }

    // Step 3: partition items into per-module groups.
    // Items before the first boundary → remaining entry.
    // Items between consecutive boundaries → one module each.
    // Items after the last module's range → remaining entry (tail).
    let mut modules = Vec::new();
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut seen_names: HashMap<String, usize> = HashMap::new();

    // Also mark the __export helper declaration as consumed so it doesn't
    // appear in every extracted module or the remaining entry.
    if let Some(idx) = find_export_helper_index(items, &export_helper) {
        consumed.insert(idx);
    }

    for (bi, boundary) in boundaries.iter().enumerate() {
        // Module range: from the namespace decl to just before the next namespace decl.
        let start = boundary.ns_decl_index;
        let end = if bi + 1 < boundaries.len() {
            boundaries[bi + 1].ns_decl_index
        } else {
            find_last_module_end(
                items,
                boundary.export_call_index + 1,
                &boundary.exported_bindings,
            )
        };

        // Collect items for this module (skip the ns decl and __export call themselves).
        let mut body_items: Vec<ModuleItem> = Vec::new();
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            if i == boundary.ns_decl_index || i == boundary.export_call_index {
                consumed.insert(i);
                continue;
            }
            consumed.insert(i);
            body_items.push(item.clone());
        }

        if body_items.is_empty() {
            continue;
        }

        let base_name = boundary.ns_atom.to_string();
        let count = seen_names.entry(base_name.clone()).or_insert(0);
        *count += 1;
        let unique_name = if *count == 1 {
            base_name
        } else {
            format!("{}_{}", base_name, count)
        };

        let filename = format!("{unique_name}.js");
        let code = emit_items(body_items, filename.clone(), cm.clone());
        modules.push(UnpackedModule {
            id: unique_name,
            is_entry: false,
            code,
            filename,
        });
    }

    // Remaining: items not consumed by any module.
    let remaining: Vec<ModuleItem> = items
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed.contains(i))
        .map(|(_, item)| item.clone())
        .collect();

    (modules, remaining)
}

struct ScopeHoistedBoundary {
    ns_atom: Atom,
    ns_decl_index: usize,
    export_call_index: usize,
    exported_bindings: HashSet<Atom>,
}

/// Detect the `__export` helper: an arrow with 2 params whose body is a
/// single for-in loop (iterating over the second param).
fn detect_export_helper(items: &[ModuleItem]) -> Option<Atom> {
    for item in items {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(bi) = &decl.name else { continue };
            let Some(init) = &decl.init else { continue };
            if is_export_helper(init) {
                return Some(bi.id.sym.clone());
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

/// Find the item index of the __export helper declaration.
fn find_export_helper_index(items: &[ModuleItem], helper_sym: &Atom) -> Option<usize> {
    items.iter().position(|item| {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            return false;
        };
        var.decls.iter().any(|d| {
            let Pat::Ident(bi) = &d.name else {
                return false;
            };
            bi.id.sym == *helper_sym
        })
    })
}

/// Find all namespace + __export call pairs.
/// Pattern: `var NS = {};` at index i, `__export(NS, { ... })` at index i+1.
fn collect_scope_hoisted_boundaries(
    items: &[ModuleItem],
    export_helper: &Atom,
) -> Vec<ScopeHoistedBoundary> {
    let mut boundaries = Vec::new();

    for i in 0..items.len().saturating_sub(1) {
        // Check: var NS = {};
        let Some(ns_atom) = extract_empty_object_decl(&items[i]) else {
            continue;
        };

        // Check: __export(NS, { ... }) at i+1
        if !is_export_call(&items[i + 1], export_helper, &ns_atom) {
            continue;
        }

        let exported_bindings = extract_export_bindings(&items[i + 1]);

        boundaries.push(ScopeHoistedBoundary {
            ns_atom,
            ns_decl_index: i,
            export_call_index: i + 1,
            exported_bindings,
        });
    }

    boundaries
}

/// Check if a namespace atom appears in any ESM export declaration.
/// e.g. `export { math_exports as math }` contains the ident `math_exports`.
fn namespace_is_module_exported(items: &[ModuleItem], ns_atom: &Atom) -> bool {
    items.iter().any(|item| {
        matches!(item, ModuleItem::ModuleDecl(_))
            && items_reference_name(std::slice::from_ref(item), ns_atom)
    })
}

/// Extract the binding atoms from `__export(NS, { key: () => binding, ... })`.
fn extract_export_bindings(item: &ModuleItem) -> HashSet<Atom> {
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
                bindings.insert(id.sym.clone());
            }
        }
    }
    bindings
}

/// Extract the binding atom from `var X = {};` (single declarator, empty object init).
fn extract_empty_object_decl(item: &ModuleItem) -> Option<Atom> {
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
    Some(bi.id.sym.clone())
}

/// Check if an item is `__export(NS, { ... })`.
fn is_export_call(item: &ModuleItem, export_helper: &Atom, ns_atom: &Atom) -> bool {
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
    if callee_id.sym != *export_helper || call.args.len() != 2 {
        return false;
    }
    // First arg must be the namespace ident.
    let Expr::Ident(first_arg) = &*call.args[0].expr else {
        return false;
    };
    if first_arg.sym != *ns_atom {
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
    from: usize,
    exported_bindings: &HashSet<Atom>,
) -> usize {
    // Phase 1: find the last item that declares an exported binding.
    let mut last_export_idx = None;
    for (i, item) in items.iter().enumerate().skip(from) {
        if matches!(item, ModuleItem::ModuleDecl(_)) {
            break;
        }
        if let Some(names) = item_declared_names(item) {
            if names.iter().any(|n| exported_bindings.contains(n)) {
                last_export_idx = Some(i);
            }
        }
    }

    let Some(last) = last_export_idx else {
        return from;
    };

    // Phase 2: reference closure — include declarations whose names are
    // referenced by the module code collected so far. This captures private
    // helpers that esbuild emits after the exported functions that call them.
    let mut end = last + 1;
    let mut module_bindings: HashSet<Atom> = exported_bindings.clone();
    while end < items.len() {
        let item = &items[end];
        if matches!(item, ModuleItem::ModuleDecl(_)) {
            break;
        }
        let Some(names) = item_declared_names(item) else {
            break;
        };
        if !names
            .iter()
            .any(|n| items_reference_name(&items[from..end], n))
        {
            break;
        }

        for n in &names {
            module_bindings.insert(n.clone());
        }
        end += 1;
    }

    // Phase 3: include trailing expression statements that reference any
    // binding from this module (side effects like `register("self", ...)`
    // or `console.log(value)`). Stop at expressions that only reference
    // globals/literals, declarations, or ModuleDecls.
    for (i, item) in items.iter().enumerate().skip(end) {
        match item {
            ModuleItem::ModuleDecl(_) => return i,
            ModuleItem::Stmt(Stmt::Expr(expr_stmt)) => {
                if !expr_references_any_binding(&expr_stmt.expr, &module_bindings) {
                    return i;
                }
            }
            ModuleItem::Stmt(Stmt::Decl(_)) => return i,
            _ => return i,
        }
    }
    items.len()
}

/// Check if any item in the slice references `name` as an identifier.
fn items_reference_name(items: &[ModuleItem], name: &Atom) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct NameFinder<'a> {
        name: &'a Atom,
        found: bool,
    }

    impl Visit for NameFinder<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if ident.sym == *self.name {
                self.found = true;
            }
        }
    }

    let mut finder = NameFinder { name, found: false };
    for item in items {
        item.visit_with(&mut finder);
        if finder.found {
            return true;
        }
    }
    false
}

/// Check if an expression references any binding from the given set.
fn expr_references_any_binding(expr: &Expr, bindings: &HashSet<Atom>) -> bool {
    use swc_core::ecma::visit::{Visit, VisitWith};

    struct BindingRefFinder<'a> {
        bindings: &'a HashSet<Atom>,
        found: bool,
    }

    impl Visit for BindingRefFinder<'_> {
        fn visit_ident(&mut self, ident: &swc_core::ecma::ast::Ident) {
            if self.bindings.contains(&ident.sym) {
                self.found = true;
            }
        }
    }

    let mut finder = BindingRefFinder {
        bindings,
        found: false,
    };
    expr.visit_with(&mut finder);
    finder.found
}

/// Extract all declared binding names from a statement.
fn item_declared_names(item: &ModuleItem) -> Option<Vec<Atom>> {
    match item {
        ModuleItem::Stmt(Stmt::Decl(Decl::Fn(f))) => Some(vec![f.ident.sym.clone()]),
        ModuleItem::Stmt(Stmt::Decl(Decl::Class(c))) => Some(vec![c.ident.sym.clone()]),
        ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
            let names: Vec<Atom> = var
                .decls
                .iter()
                .filter_map(|d| {
                    if let Pat::Ident(bi) = &d.name {
                        Some(bi.id.sym.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if names.is_empty() {
                None
            } else {
                Some(names)
            }
        }
        _ => None,
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
