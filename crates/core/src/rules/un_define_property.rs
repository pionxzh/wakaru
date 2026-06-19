//! Detect and unwrap the Babel `_defineProperty` helper when it is inlined as
//! a local function decl.
//!
//! Shape matched:
//!
//! ```js
//! function _defineProperty(e, t, n) {
//!     if (t in e) {
//!         Object.defineProperty(e, t, {
//!             value: n,
//!             enumerable: true,
//!             configurable: true,
//!             writable: true,
//!         });
//!     } else {
//!         e[t] = n;
//!     }
//!     return e;
//! }
//! ```
//!
//! Call sites rewritten only when the call is a standalone expression
//! statement (the helper's return value is discarded). Call-in-expression
//! positions like `const x = _defineProperty(o, k, v)` are left alone —
//! rewriting them would lose the `return e` semantics.
//!
//! If all references to the helper are call sites we rewrote, the helper
//! function declaration is dropped. Exported helpers are always preserved.

use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Callee, ComputedPropName, Expr, ExprStmt, MemberExpr,
    MemberProp, Module, ModuleDecl, ModuleItem, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, fn_decl_binding_key, import_specifier_binding_key,
    remaining_refs_outside_skipped_items, remove_fn_decls_by_binding,
    remove_import_specifiers_by_binding, BindingKey,
};
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};

pub struct UnDefineProperty;

impl UnDefineProperty {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_define_property(module, local_helpers);
    }
}

impl VisitMut for UnDefineProperty {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_define_property(module, &local_helpers);
    }
}

fn run_un_define_property(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers: HelperSet = local_helpers
        .helpers_of_kind(TranspilerHelperKind::DefineProperty)
        .into_keys()
        .collect();
    if helpers.is_empty() {
        return;
    }

    let mut rewriter = CallSiteRewriter {
        helpers: &helpers,
        rewrote_any: false,
    };
    module.visit_mut_with(&mut rewriter);

    if rewriter.rewrote_any {
        remove_unused_helpers(module, &helpers);
    }
}

/// Helper declaration bindings, keyed by resolver binding identity.
type HelperSet = HashSet<BindingKey>;

struct CallSiteRewriter<'a> {
    helpers: &'a HelperSet,
    rewrote_any: bool,
}

impl CallSiteRewriter<'_> {
    fn try_rewrite_expr_stmt(&mut self, stmt: &mut Stmt) -> bool {
        let Stmt::Expr(ExprStmt { expr, span }) = stmt else {
            return false;
        };
        let Expr::Call(call) = expr.as_ref() else {
            return false;
        };
        let Callee::Expr(callee_expr) = &call.callee else {
            return false;
        };
        let Expr::Ident(callee_ident) = callee_expr.as_ref() else {
            return false;
        };
        if !self.helpers.contains(&binding_key(callee_ident)) {
            return false;
        }
        if call.args.len() != 3 {
            return false;
        }
        // No spread args allowed — we need all three positionally.
        if call.args.iter().any(|a| a.spread.is_some()) {
            return false;
        }

        let obj = call.args[0].expr.clone();
        let key = call.args[1].expr.clone();
        let value = call.args[2].expr.clone();

        // Build obj[key] = value
        let new_expr = Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Member(MemberExpr {
                span: DUMMY_SP,
                obj,
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: key,
                }),
            })),
            right: value,
        });
        *stmt = Stmt::Expr(ExprStmt {
            span: *span,
            expr: Box::new(new_expr),
        });
        self.rewrote_any = true;
        true
    }
}

impl VisitMut for CallSiteRewriter<'_> {
    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        self.try_rewrite_expr_stmt(stmt);
        stmt.visit_mut_children_with(self);
    }
}

fn remove_unused_helpers(module: &mut Module, helpers: &HelperSet) {
    let remaining = remaining_refs_outside_skipped_items(module, helpers, |item| {
        if fn_decl_binding_key(item).is_some_and(|key| helpers.contains(&key)) {
            return true;
        }
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            if import.specifiers.len() == 1 {
                let key = import_specifier_binding_key(&import.specifiers[0]);
                return helpers.contains(&key);
            }
        }
        false
    });
    let removable: HashSet<_> = helpers.difference(&remaining).cloned().collect();
    remove_fn_decls_by_binding(module, &removable);
    remove_import_specifiers_by_binding(&mut module.body, &removable);
}
