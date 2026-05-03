use std::collections::HashMap;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Callee, Decl, Expr, Ident, ImportDecl, ImportStarAsSpecifier, Lit, Module, ModuleDecl,
    ModuleItem, Pat, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, remove_helper_declarations, BabelHelperKind, BindingKey,
};

/// Detects and unwraps `interopRequireWildcard` helper calls.
///
/// Transforms:
///   `var _a = _interopRequireWildcard(require("a"))`
///   → `import * as _a from "a"`
///
/// Also handles the 2-arg form: `_irw(require("a"), true)` → `import * as _a from "a"`
///
/// For non-require arguments, just unwraps: `_irw(expr)` → `expr`
pub struct UnInteropRequireWildcard;

impl VisitMut for UnInteropRequireWildcard {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| *kind == BabelHelperKind::InteropRequireWildcard)
            .collect();
        if helpers.is_empty() {
            return;
        }

        // Phase 1: Convert `var _a = _irw(require("path"))` → `import * as _a from "path"`
        // and unwrap non-require calls in expressions.
        let mut new_body = Vec::with_capacity(module.body.len());
        for item in module.body.drain(..) {
            if let Some(imports) = try_convert_to_namespace_import(&item, &helpers) {
                new_body.extend(imports);
            } else {
                new_body.push(item);
            }
        }
        module.body = new_body;

        // Phase 2: Unwrap remaining call sites in expressions (non-var-decl contexts)
        let mut unwrapper = WildcardCallUnwrapper { helpers: &helpers };
        module.visit_mut_with(&mut unwrapper);

        // Phase 3: Remove helper declarations.
        remove_helper_declarations(&mut module.body, &helpers);
    }
}

/// Try to convert a `var _x = _irw(require("path"))` into `import * as _x from "path"`.
/// Returns None if the item doesn't match this pattern.
fn try_convert_to_namespace_import(
    item: &ModuleItem,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> Option<Vec<ModuleItem>> {
    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
        return None;
    };

    let mut result = Vec::new();
    let mut remaining_decls = Vec::new();
    let mut had_conversion = false;

    for decl in &var.decls {
        let Pat::Ident(bi) = &decl.name else {
            remaining_decls.push(decl.clone());
            continue;
        };
        let Some(init) = &decl.init else {
            remaining_decls.push(decl.clone());
            continue;
        };

        if let Some(source) = extract_wildcard_require(init, helpers) {
            // Convert to: import * as _x from "source"
            let import = ImportDecl {
                span: DUMMY_SP,
                specifiers: vec![swc_core::ecma::ast::ImportSpecifier::Namespace(
                    ImportStarAsSpecifier {
                        span: DUMMY_SP,
                        local: Ident::new(bi.id.sym.clone(), DUMMY_SP, bi.id.ctxt),
                    },
                )],
                src: Box::new(source),
                type_only: false,
                with: None,
                phase: Default::default(),
            };
            result.push(ModuleItem::ModuleDecl(ModuleDecl::Import(import)));
            had_conversion = true;
        } else {
            remaining_decls.push(decl.clone());
        }
    }

    if !had_conversion {
        return None;
    }

    // Keep any remaining declarators in the var statement
    if !remaining_decls.is_empty() {
        let mut var = var.clone();
        var.decls = remaining_decls;
        result.push(ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))));
    }

    Some(result)
}

/// Extract the require source from `_irw(require("path"))` or `_irw(require("path"), true)`.
fn extract_wildcard_require(
    expr: &Expr,
    helpers: &HashMap<BindingKey, BabelHelperKind>,
) -> Option<swc_core::ecma::ast::Str> {
    let Expr::Call(call) = expr else { return None };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return None;
    };

    if !helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
        return None;
    }

    if call.args.is_empty() || call.args.len() > 2 {
        return None;
    }

    // First arg must be require("path")
    let Expr::Call(require_call) = call.args[0].expr.as_ref() else {
        return None;
    };
    let Callee::Expr(require_callee) = &require_call.callee else {
        return None;
    };
    let Expr::Ident(require_id) = require_callee.as_ref() else {
        return None;
    };
    if require_id.sym.as_ref() != "require" || require_call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(Lit::Str(source)) = require_call.args[0].expr.as_ref() else {
        return None;
    };

    Some(source.clone())
}

/// Unwrap remaining wildcard calls in non-var-decl contexts,
/// but only when the argument is a `require()` call.
/// Non-require arguments are left as-is because the helper synthesizes
/// a namespace object that may differ from the raw expression value.
struct WildcardCallUnwrapper<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for WildcardCallUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        if !self.helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
            return;
        }

        if call.args.is_empty() || call.args.len() > 2 {
            return;
        }

        // Only unwrap when the first arg is require("...")
        if is_require_call(&call.args[0].expr) {
            *expr = *call.args[0].expr.clone();
        }
    }
}

fn is_require_call(expr: &Expr) -> bool {
    let Expr::Call(call) = expr else { return false };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    id.sym.as_ref() == "require" && call.args.len() == 1
}
