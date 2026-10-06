//! Namespace imports for relative requires of a module compiled from ESM,
//! when no provider facts exist (single-file decompilation).
//!
//! `UnEsm` turns `var dep = require("./dep")` into `import dep from "./dep"`,
//! which works for any CommonJS provider but fails to link against an ESM
//! provider without a default export. Babel, TypeScript, swc, sucrase, and
//! esbuild lower `import { x } from "./dep"` to exactly that `require` plus
//! `dep.x` reads; a default import reads `dep.default` or wraps the module in
//! an interop helper. So in a module compiled from ESM, a relative `require`
//! binding with neither is a named import, and its provider most likely a
//! sibling compiled the same way: `import * as dep` matches the source.
//!
//! The evidence has to be collected before `UnEsm` unwraps interop calls,
//! which rewrites `_interopRequireDefault(require("./a")).default.x` to a
//! plain `require("./a")` binding read as `.x`, the same shape as a named
//! import, and removes the `__esModule` marker of a module it converts. The
//! `UnEsm` runner collects it, runs `UnEsm`, and then applies this rewrite.
//! esbuild has no marker; `UnEsm` records its `__toCommonJS` namespace
//! instead.
//!
//! In unpack mode the provider facts make this decision
//! (`provider_namespace_repair`), so the rewrite runs only without facts. See
//! `relative_require_esm_provider` in `docs/rewrite-assumptions.md`.

use crate::collections::HashSet;

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    CallExpr, Callee, Decl, Expr, Lit, MemberExpr, MemberProp, Module, ModuleItem, Pat, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};
use super::un_esmodule_flag::has_top_level_esmodule_flag;
use crate::analysis::{binding_id, BindingId};
use crate::provider_namespace_repair::run_relative_namespace_repair;
use crate::rules::expr_utils::is_unresolved_ident;
use crate::utils::paren::strip_parens;

/// What the module showed before `UnEsm` erased it.
#[derive(Debug, Default)]
pub(crate) struct RelativeNamespaceEvidence {
    /// A top-level `__esModule` marker, or an esbuild `__toCommonJS`
    /// namespace lowered by `UnEsm`.
    pub(crate) compiled_from_esm: bool,
    /// Top-level `x = require("./...")` bindings with no default-import sign.
    pub(crate) bindings: HashSet<BindingId>,
}

impl RelativeNamespaceEvidence {
    pub(crate) fn collect(
        module: &Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
    ) -> Self {
        let mut bindings = HashSet::default();
        for item in &module.body {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                continue;
            };
            for declarator in &var.decls {
                let Pat::Ident(name) = &declarator.name else {
                    continue;
                };
                if declarator
                    .init
                    .as_deref()
                    .is_some_and(|init| is_relative_require(init, unresolved_mark))
                {
                    bindings.insert(binding_id(&name.id));
                }
            }
        }
        if bindings.is_empty() {
            return Self::default();
        }

        let mut finder = DefaultImportSigns {
            candidates: &bindings,
            local_helpers,
            excluded: HashSet::default(),
        };
        module.visit_with(&mut finder);
        let excluded = finder.excluded;
        bindings.retain(|binding| !excluded.contains(binding));
        Self {
            compiled_from_esm: has_top_level_esmodule_flag(module, unresolved_mark),
            bindings,
        }
    }
}

/// Rewrite the synthesized default imports of the collected bindings to
/// namespace imports, when the module was compiled from ESM.
pub(crate) fn run_relative_namespace_import(
    module: &mut Module,
    evidence: &RelativeNamespaceEvidence,
    unresolved_mark: Mark,
) {
    if evidence.compiled_from_esm && !evidence.bindings.is_empty() {
        run_relative_namespace_repair(module, unresolved_mark, &evidence.bindings);
    }
}

/// `require("./x")` or `require("../x")`, called directly.
fn is_relative_require(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let [arg] = call.args.as_slice() else {
        return false;
    };
    matches!(strip_parens(callee), Expr::Ident(id) if is_unresolved_ident(id, "require", unresolved_mark))
        && arg.spread.is_none()
        && matches!(arg.expr.as_ref(), Expr::Lit(Lit::Str(source))
            if source.value.as_str().is_some_and(is_relative_source))
}

pub(crate) fn is_relative_source(source: &str) -> bool {
    source.starts_with("./") || source.starts_with("../")
}

/// Marks candidates read as `x.default` or passed to an interop-default
/// helper: both are how compilers lower a default import.
struct DefaultImportSigns<'a> {
    candidates: &'a HashSet<BindingId>,
    local_helpers: &'a LocalHelperContext,
    excluded: HashSet<BindingId>,
}

impl DefaultImportSigns<'_> {
    fn candidate(&self, expr: &Expr) -> Option<BindingId> {
        let Expr::Ident(ident) = strip_parens(expr) else {
            return None;
        };
        let binding = binding_id(ident);
        self.candidates.contains(&binding).then_some(binding)
    }
}

impl Visit for DefaultImportSigns<'_> {
    fn visit_member_expr(&mut self, member: &MemberExpr) {
        let reads_default = match &member.prop {
            MemberProp::Ident(property) => property.sym == "default",
            MemberProp::Computed(computed) => matches!(computed.expr.as_ref(),
                Expr::Lit(Lit::Str(name)) if name.value.as_str() == Some("default")),
            MemberProp::PrivateName(_) => false,
        };
        if reads_default {
            if let Some(binding) = self.candidate(&member.obj) {
                self.excluded.insert(binding);
            }
        }
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if self
                .local_helpers
                .is_helper_callee(callee, TranspilerHelperKind::InteropRequireDefault)
            {
                for arg in &call.args {
                    if let Some(binding) = self.candidate(&arg.expr) {
                        self.excluded.insert(binding);
                    }
                }
            }
        }
        call.visit_children_with(self);
    }
}
