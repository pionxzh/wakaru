use std::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, GLOBALS};
use swc_core::ecma::ast::{
    ArrowExpr, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, ExprStmt, Module, ModuleItem, Pat,
    Stmt, Str, VarDeclarator,
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
    if helper_syms.is_empty() {
        return None;
    }

    // Phase 2: collect factory declarations — `var X = helper(factory_fn)`.
    let factories = collect_factories(module, &helper_syms);

    // Require a meaningful number of factories to avoid false positives on code that
    // happens to have a couple of arrow-returning-arrow helpers.
    if factories.len() < 5 {
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

    if !entry_items.is_empty() {
        let entry_module = Module {
            span: Default::default(),
            body: entry_items,
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

