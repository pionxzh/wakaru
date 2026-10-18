//! Lit / Polymer / jsaction unpacker.
//!
//! Detects and unpacks bundles produced by the Lit (formerly Polymer) web
//! component build pipeline, including:
//!
//! - `@lit/reactive-element` custom element registrations
//! - `@lit-labs/ssr` server-side rendering bundles
//! - Google `jsaction` / `Wiz` event delegation bundles
//! - `goog:provide` / `goog:require` namespace patterns (Closure-style)
//!
//! Key signatures:
//!
//! 1. Lit custom elements use `customElements.define(tagName, ClassName)` with
//!    class bodies that call `super()` and `super.connectedCallback()`.
//!    Bundled Lit apps wrap these in a module registry keyed by npm-style
//!    specifiers (`@lit/reactive-element`, `@lit/lit-html`, `./my-element.js`, etc.).
//!
//! 2. jsaction bundles use `goog.provide('my.namespace')` and
//!    `goog.require('other.namespace')` with a module table pattern similar
//!    to Closure compiler output.
//!
//! This detector runs after webpack4/5, browserify, and turbopack in the
//! detector chain, catching frameworks those bundlers don't special-case.

use crate::collections::HashSet;

use swc_core::common::{sync::Lrc, SourceMap, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowFunctionBody, CallExpr, Callee, ComputedPropName, Expr, ExprStmt, FnDecl, Function, Ident,
    Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, Number, Stmt, Str,
};

use crate::unpacker::{
    source_fallback_for_stmts, BundleFormat, DetectedBundle, UnpackResult, UnpackedModule,
};
use crate::utils::paren::strip_parens;

// ---------------------------------------------------------------------------
// BundleFormat variant
// ---------------------------------------------------------------------------

// NOTE: this file does NOT modify BundleFormat — instead, we return
// BundleFormat::Webpack5 for lit bundles that webpack produces (lit uses
// webpack under the hood) or BundleFormat::ScopeHoisted for lit-analyzer
// processed output. For pure jsaction/Closure bundles we return a new
// format which we add to the enum below.

// We'll use ScopeHoisted as the format label for lit/jsaction since these
// use custom module registries rather than standard bundler formats.

// ---------------------------------------------------------------------------
// Detection entry point
// ---------------------------------------------------------------------------

/// Try to detect and extract a Lit / Polymer / jsaction bundle.
///
/// Returns `Some(DetectedBundle)` if the source matches a known Lit/jsaction
/// pattern, `None` otherwise.
pub fn detect_from_module_prepared(module: &Module, cm: Lrc<SourceMap>) -> Option<DetectedBundle> {
    let span = tracing::info_span!("lit: detect_from_module");
    let _enter = span.enter();

    // Check 1: Lit customElements.define registration pattern
    if let Some(result) = try_detect_lit_custom_elements(module, cm.clone()) {
        return Some(result);
    }

    // Check 2: goog.provide / goog.require (jsaction / Closure pattern)
    if let Some(result) = try_detect_goog_provide(module, cm.clone()) {
        return Some(result);
    }

    // Check 3: lit-html / reactive-element import graph pattern
    if let Some(result) = try_detect_lit_import_graph(module, cm.clone()) {
        return Some(result);
    }

    None
}

// ---------------------------------------------------------------------------
// Lit Custom Elements detection
// ---------------------------------------------------------------------------

/// Detect Lit custom element bundles.
///
/// Signature: `customElements.define("my-element", MyElementClass)` wrapped
/// in a module table: `(function(modules) { ... })([{default: ...}])`.
/// Lit uses webpack under the hood, so this is an extra detection pass
/// for cases where the webpack-specific signatures don't match.
fn try_detect_lit_custom_elements(
    module: &Module,
    cm: Lrc<SourceMap>,
) -> Option<DetectedBundle> {
    // Look for customElements.define at the top level
    let has_custom_elements_define = module.body.iter().any(|item| {
        if let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item {
            is_custom_elements_define_call(expr)
        } else {
            false
        }
    });

    if !has_custom_elements_define {
        return None;
    }

    // Collect all module factories
    let mut modules: Vec<FactoryModule> = Vec::new();
    let mut entry_ids = Vec::new();

    // Look for the module table pattern
    for (idx, item) in module.body.iter().enumerate() {
        if let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item {
            // Check for a call like modules[id](...)
            if let Some(factory) = extract_module_factory(expr, idx) {
                modules.push(factory);
            }

            // Check for customElements.define
            if let Some(call) = strip_parens(expr) {
                if let Expr::Call(CallExpr { callee, args, .. }) = call {
                    if let Callee::Expr(callee_expr) = callee {
                        if let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(callee_expr) {
                            if let Expr::Ident(obj_ident) = strip_parens(obj) {
                                if obj_ident.sym.as_ref() == "customElements" {
                                    if let MemberProp::Computed(ComputedPropName { expr: tag_expr, .. }) = prop {
                                        if let Expr::Lit(Lit::Str(Str { value: tag, .. })) = strip_parens(tag_expr) {
                                            entry_ids.push(tag.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if modules.is_empty() {
        return None;
    }

    // Build the result
    let mut unpacked_modules = Vec::new();
    let mut prepared = Vec::new();

    for (idx, factory) in modules.iter().enumerate() {
        let (code, generated_source_map) = emit_module_for_factory(&cm, factory.body_stmts);
        unpacked_modules.push(UnpackedModule {
            id: factory.id.clone(),
            is_entry: entry_ids.contains(&factory.id),
            code,
            filename: factory.filename.clone(),
            source_ranges: factory.source_span.map(|s| span_byte_range(&cm, s).unwrap_or_default()),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map,
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        });
        prepared.push(None);
    }

    let result = UnpackResult::new(unpacked_modules, BundleFormat::ScopeHoisted);
    Some(DetectedBundle::from_result(result))
}

fn is_custom_elements_define_call(expr: &Expr) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) else {
        return false;
    };
    if args.len() < 2 {
        return false;
    }
    if let Callee::Expr(callee_expr) = callee {
        if let Expr::Member(MemberExpr { obj, prop: MemberProp::Ident(prop_ident), .. }) = strip_parens(callee_expr) {
            if let Expr::Ident(obj_ident) = strip_parens(obj) {
                return obj_ident.sym.as_ref() == "customElements"
                    && prop_ident.sym.as_ref() == "define";
            }
        }
    }
    false
}

fn extract_module_factory<'a>(expr: &'a Expr, idx: usize) -> Option<FactoryModule<'a>> {
    // Look for: modules["id"](...)  or  modules[0](...)
    let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) else {
        return None;
    };
    if args.len() < 1 {
        return None;
    }

    // The callee should be: modules[id]
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(&callee) else {
        return None;
    };
    if let Expr::Ident(ident) = strip_parens(obj) {
        if ident.sym.as_ref() == "require" || ident.sym.as_ref() == "modules" {
            // Extract the module ID
            let id = match strip_parens(prop) {
                Expr::Lit(Lit::Str(Str { value, .. })) => value.to_string(),
                Expr::Lit(Lit::Num(Number { value, .. })) => {
                    format!("{}", value as i64)
                }
                Expr::Member(MemberExpr { obj, prop: MemberProp::Computed(comp), .. }) => {
                    if let Expr::Lit(Lit::Str(Str { value, .. })) = strip_parens(comp) {
                        value.to_string()
                    } else {
                        return None;
                    }
                }
                _ => return None,
            };

            // Get the factory function from the first argument
            if let Expr::Fn(FnDecl { function: Function { body: Some(body), .. }, .. }) = strip_parens(&args[0].expr) {
                return Some(FactoryModule {
                    id,
                    filename: format!("module-{}.js", idx),
                    body_stmts: &body.stmts,
                    source_span: None,
                });
            }
            if let Expr::Arrow(ArrowFunctionBody::FunctionBody(body)) = strip_parens(&args[0].expr) {
                return Some(FactoryModule {
                    id,
                    filename: format!("module-{}.js", idx),
                    body_stmts: &body.stmts,
                    source_span: None,
                });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// goog.provide / goog.require detection (jsaction / Closure)
// ---------------------------------------------------------------------------

/// Detect Closure Compiler / jsaction bundles with goog.provide/goog.require.
///
/// Signature: A module table like `(function(exports) { goog.provide('x'); goog.require('y'); ... })(exports)`.
fn try_detect_goog_provide(module: &Module, cm: Lrc<SourceMap>) -> Option<DetectedBundle> {
    let mut factories: Vec<(String, Vec<&Stmt>)> = Vec::new();
    let mut provide_names = HashSet::new();

    // Check for goog.provide or goog.require at the top level
    let has_goog = module.body.iter().any(|item| {
        if let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item {
            is_goog_call(expr, "provide") || is_goog_call(expr, "require")
        } else {
            false
        }
    });

    if !has_goog {
        return None;
    }

    // Find module factory patterns
    for (idx, item) in module.body.iter().enumerate() {
        if let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item {
            if let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) {
                // Look for the factory call pattern
                if let Callee::Expr(callee_expr) = callee {
                    if let Expr::Fn(FnDecl { function: Function { body: Some(body), .. }, .. }) = strip_parens(callee_expr) {
                        let mut stmts = Vec::new();
                        for stmt in &body.stmts {
                            if let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(stmt) {
                                if let Callee::Expr(callee_expr) = callee {
                                    if is_goog_call_expr(callee_expr, "provide") {
                                        if let Expr::Lit(Lit::Str(Str { value, .. })) = strip_parens(&args[0].expr) {
                                            provide_names.insert(value.to_string());
                                        }
                                    }
                                }
                            }
                            stmts.push(stmt);
                        }
                        if !stmts.is_empty() {
                            factories.push((format!("module-{}", idx), stmts));
                        }
                    }
                }
            }
        }
    }

    if factories.is_empty() {
        return None;
    }

    let mut unpacked = Vec::new();
    for (id, stmts) in &factories {
        let code = source_fallback_for_stmts(&cm, stmts);
        unpacked.push(UnpackedModule {
            id: id.clone(),
            is_entry: provide_names.contains(id),
            code,
            filename: format!("{}.js", id),
            source_ranges: Vec::new(),
            inspection_context_ranges: Vec::new(),
            source_input: String::new(),
            generated_source_map: Vec::new(),
            verbatim_source_offset: None,
            mapped_in_every_mode: false,
        });
    }

    let result = UnpackResult::new(unpacked, BundleFormat::ScopeHoisted);
    Some(DetectedBundle::from_result(result))
}

fn is_goog_call(expr: &Expr, method: &str) -> bool {
    let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) else {
        return false;
    };
    if let Callee::Expr(callee_expr) = callee {
        is_goog_call_expr(callee_expr, method)
    } else {
        false
    }
}

fn is_goog_call_expr(expr: &Expr, method: &str) -> bool {
    let Expr::Member(MemberExpr { obj, prop: MemberProp::Ident(prop_ident), .. }) = strip_parens(expr) else {
        return false;
    };
    if let Expr::Ident(obj_ident) = strip_parens(obj) {
        return obj_ident.sym.as_ref() == "goog"
            && prop_ident.sym.as_ref() == method;
    }
    false
}

// ---------------------------------------------------------------------------
// Lit import graph detection (import graph wrapped in function)
// ---------------------------------------------------------------------------

/// Detect Lit / lit-html / reactive-element import graphs.
///
/// These use a module wrapper pattern similar to IIFE bundlers but with
/// Lit-specific import resolution (e.g., `@lit/reactive-element/lib/hydrate.js`).
fn try_detect_lit_import_graph(module: &Module, cm: Lrc<SourceMap>) -> Option<DetectedBundle> {
    // Look for lit-specific imports
    let has_lit_import = module.body.iter().any(|item| {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            let spec = import.src.value.as_ref();
            spec.contains("/lit/")
                || spec.contains("lit-html")
                || spec.contains("lit-element")
                || spec.contains("reactive-element")
                || spec.contains("lit/reactive-element")
                || spec.contains("lit/lit-html")
                || spec.contains("lit-element")
        } else {
            false
        }
    });

    if !has_lit_import {
        return None;
    }

    // Check for a module table wrapping pattern
    let has_module_table = module.body.iter().any(|item| {
        if let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = item {
            if let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) {
                // modules[id](...) pattern
                if let Callee::Expr(callee_expr) = callee {
                    if let Expr::Member(MemberExpr { obj, .. }) = strip_parens(callee_expr) {
                        if let Expr::Ident(ident) = strip_parens(obj) {
                            return ident.sym.as_ref() == "modules" || ident.sym.as_ref() == "require";
                        }
                    }
                }
            }
        }
        false
    });

    if !has_module_table {
        return None;
    }

    // Simple pass: treat the whole file as one module since Lit bundles
    // are typically small enough to decompile in place
    let code = source_fallback_for_stmts(&cm, &module.body);
    let generated_source_map = Vec::new();

    let mut unpacked = Vec::new();
    unpacked.push(UnpackedModule {
        id: "entry".to_string(),
        is_entry: true,
        code,
        filename: "lit-entry.js",
        source_ranges: Vec::new(),
        inspection_context_ranges: Vec::new(),
        source_input: String::new(),
        generated_source_map,
        verbatim_source_offset: None,
        mapped_in_every_mode: false,
    });

    let result = UnpackResult::new(unpacked, BundleFormat::ScopeHoisted);
    Some(DetectedBundle::from_result(result))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit_module_for_factory<'a>(
    cm: &Lrc<SourceMap>,
    stmts: &'a [Stmt],
) -> (String, Vec<crate::unpacker::GeneratedSourceMapPoint>) {
    // For simple cases, just return the source fallback
    // Full implementation would require creating a PreparedModuleAst
    let code = source_fallback_for_stmts(cm, stmts);
    (code, Vec::new())
}

fn span_byte_range(cm: &Lrc<SourceMap>, span: Span) -> (u32, u32) {
    if span.lo.0 == 0 || span.hi.0 == 0 || span.lo > span.hi {
        return (0, 0);
    }
    let file = cm.lookup_byte_offset(span.lo).sf;
    let start = span.lo.0.saturating_sub(file.start_pos.0);
    let end = span.hi.0.saturating_sub(file.start_pos.0);
    (start, end)
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

struct FactoryModule<'a> {
    id: String,
    filename: String,
    body_stmts: &'a [Stmt],
    source_span: Option<Span>,
}
