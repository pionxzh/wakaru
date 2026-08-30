use std::path::Path;

use swc_core::ecma::ast::{
    Expr, Lit, MetaPropExpr, MetaPropKind, Module, ModuleDecl, ModuleItem, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

/// Remove top-level `"use strict"` expressions only when the final output has
/// a trusted Module goal. Function-level directives are intentionally outside
/// this finalization pass and remain available to every rewrite rule.
pub(super) fn strip_redundant_module_use_strict(
    module: &mut Module,
    filename: &str,
    bare_imports_are_stable: bool,
) {
    if !is_definitely_module(module, filename, bare_imports_are_stable) {
        return;
    }

    module.body.retain(|item| {
        !matches!(
            item,
            ModuleItem::Stmt(Stmt::Expr(statement))
                if matches!(
                    statement.expr.as_ref(),
                    Expr::Lit(Lit::Str(value))
                        if value.value.as_str() == Some("use strict")
                )
        )
    });
}

fn is_definitely_module(module: &Module, filename: &str, bare_imports_are_stable: bool) -> bool {
    match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
    {
        // An explicit script extension conflicts with module-only AST syntax.
        // Preserve source rather than guessing which signal the caller meant.
        Some("cjs" | "cts") => return false,
        Some("mjs" | "mts") => return true,
        _ => {}
    }

    if module.body.iter().any(|item| match item {
        ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
            bare_imports_are_stable || !import.specifiers.is_empty()
        }
        ModuleItem::ModuleDecl(_) => true,
        ModuleItem::Stmt(_) => false,
    }) {
        return true;
    }

    let mut finder = ImportMetaFinder::default();
    module.visit_with(&mut finder);
    finder.found
}

#[derive(Default)]
struct ImportMetaFinder {
    found: bool,
}

impl Visit for ImportMetaFinder {
    fn visit_meta_prop_expr(&mut self, expression: &MetaPropExpr) {
        if expression.kind == MetaPropKind::ImportMeta {
            self.found = true;
        }
    }
}
