//! Module-level helper scan: walk top-level declarations (function decls,
//! function-assigned vars, helper imports) and collect their binding identities
//! and kinds. The per-node recognition lives in `matchers`; this is the driver.

use std::collections::HashMap;

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    Callee, Decl, Expr, ImportSpecifier, Lit, Module, ModuleDecl, ModuleItem, Pat, Stmt,
    VarDeclKind,
};

use crate::analysis::binding_uses::BindingUseIndex;

use super::*;

/// Scan module-level declarations for helper functions.
/// Detects by function body shape and by import path.
pub(crate) fn collect_transpiler_helpers(
    module: &Module,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    collect_transpiler_helpers_inner(module, None)
}
pub(super) fn collect_transpiler_helpers_inner(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> HashMap<BindingKey, TranspilerHelperKind> {
    #[cfg(test)]
    COLLECT_TRANSPILER_HELPERS_CALLS.with(|calls| calls.set(calls.get() + 1));

    // Phase 1: scan all module-level function bodies for Babel sub-helper markers.
    // The Babel 7+ pattern uses a thin dispatcher (`return f(x) || g(x) || h(x) || k()`)
    // that delegates to sub-helpers defined in the same module. We only accept OR-chain
    // dispatchers when the module also contains functions with Array.isArray, Array.from,
    // or Symbol.iterator — signals that Babel sub-helpers are present.
    let has_sub_helpers = module_has_babel_sub_helper_signals(module);

    let mut helpers = HashMap::new();
    for item in &module.body {
        match item {
            // function _interopRequireDefault(obj) { ... }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                let key = binding_key(&fn_decl.ident);
                if let Some(kind) = detect_helper_from_fn(&fn_decl.function, has_sub_helpers)
                    .or_else(|| {
                        is_self_redefining_typeof_fn(&fn_decl.function, &key)
                            .then_some(TranspilerHelperKind::Typeof)
                    })
                    .or_else(|| generated_fn_helper_name_kind(fn_decl.ident.sym.as_ref()))
                {
                    helpers.insert(key, kind);
                }
            }
            // var _ird = function(obj) { ... }  OR  var _ird = require("@babel/runtime/...")
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if let Some((key, kind)) =
                        detect_helper_from_var_decl(decl, has_sub_helpers, unresolved_mark)
                    {
                        helpers.insert(key, kind);
                    }
                }
            }
            // import _extends from "@babel/runtime/helpers/extends"
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                if import.type_only {
                    continue;
                }
                let path = import.src.value.as_str().unwrap_or("");
                if is_tslib_path(path) {
                    for specifier in &import.specifiers {
                        let ImportSpecifier::Named(named) = specifier else {
                            continue;
                        };
                        let imported = named
                            .imported
                            .as_ref()
                            .map(export_name_to_atom)
                            .unwrap_or_else(|| named.local.sym.clone());
                        if let Some(kind) = tslib_helper_name_kind(imported.as_ref()) {
                            helpers.insert(binding_key(&named.local), kind);
                        }
                    }
                    continue;
                }
                let Some(kind) = detect_helper_from_path(path) else {
                    continue;
                };
                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Default(default) => {
                            helpers.insert(binding_key(&default.local), kind);
                        }
                        ImportSpecifier::Named(named) if named_import_is_helper(path, named) => {
                            helpers.insert(binding_key(&named.local), kind);
                        }
                        _ => {}
                    }
                }
            }
            // export function _extends() { ... }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Fn(fn_decl) => {
                    let key = binding_key(&fn_decl.ident);
                    if let Some(kind) = detect_helper_from_fn(&fn_decl.function, has_sub_helpers)
                        .or_else(|| {
                            is_self_redefining_typeof_fn(&fn_decl.function, &key)
                                .then_some(TranspilerHelperKind::Typeof)
                        })
                        .or_else(|| generated_fn_helper_name_kind(fn_decl.ident.sym.as_ref()))
                    {
                        helpers.insert(key, kind);
                    }
                }
                Decl::Var(var) => {
                    for decl in &var.decls {
                        if let Some((key, kind)) =
                            detect_helper_from_var_decl(decl, has_sub_helpers, unresolved_mark)
                        {
                            helpers.insert(key, kind);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    helpers
}

/// Collect the namespace bindings used by modern SWC external helpers.
///
/// SWC emits its CommonJS and AMD helper dependencies as namespace objects:
///
/// ```js
/// const helper = require("@swc/helpers/_/_interop_require_default");
/// value = helper._(value);
/// ```
///
/// Every SWC helper module exports its callable as `_`, so the member spelling
/// alone carries no helper identity. Keep this provenance separate from the
/// ordinary direct-call helper map and accept only an exact runtime require.
/// Modern targets commonly declare the namespace with `const`; older SWC and
/// lifted AMD factory parameters use `var`, which is callable only when the
/// complete binding-use index proves that namespace is never replaced.
/// Interop-default is the only consumer for now; extending this to another
/// helper kind requires that rule to handle namespace cleanup and fail-closed
/// retention too.
pub(super) fn collect_swc_member_helpers(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> (
    HashMap<BindingKey, TranspilerHelperKind>,
    HashMap<BindingKey, TranspilerHelperKind>,
) {
    let direct_writes = BindingUseIndex::collect_direct_write_bindings(module);
    let mut callable_helpers = HashMap::new();
    let mut namespaces = HashMap::new();

    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        if !matches!(var.kind, VarDeclKind::Const | VarDeclKind::Var) {
            continue;
        }
        let [decl] = var.decls.as_slice() else {
            continue;
        };
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let Some(Expr::Call(call)) = decl.init.as_deref() else {
            continue;
        };
        let Callee::Expr(callee) = &call.callee else {
            continue;
        };
        let Expr::Ident(require) = callee.as_ref() else {
            continue;
        };
        if !is_unresolved_or_unguarded_ident(require, "require", unresolved_mark) {
            continue;
        }
        let [arg] = call.args.as_slice() else {
            continue;
        };
        if arg.spread.is_some() {
            continue;
        }
        let Expr::Lit(Lit::Str(source)) = arg.expr.as_ref() else {
            continue;
        };
        let path = source.value.as_str().unwrap_or("");
        if !path.starts_with("@swc/helpers/_/_")
            || detect_helper_from_path(path) != Some(TranspilerHelperKind::InteropRequireDefault)
        {
            continue;
        }

        let key = binding_key(&binding.id);
        namespaces.insert(key.clone(), TranspilerHelperKind::InteropRequireDefault);
        if !direct_writes.contains(&key) {
            callable_helpers.insert(key, TranspilerHelperKind::InteropRequireDefault);
        }
    }

    (callable_helpers, namespaces)
}
