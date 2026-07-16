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
//! Standalone calls are rewritten to assignments when the helper's return
//! value is discarded. At standard and above, expression-position calls whose
//! target is exactly `{}` are restored to `{ [key]: value }`; other expression
//! targets stay intact because rewriting them would lose `return e` semantics.
//!
//! If all references to the helper are call sites we rewrote, the helper
//! function declaration is dropped. Exported helpers are always preserved.

use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, Callee, ComputedPropName, Expr, ExprStmt, KeyValueProp,
    MemberExpr, MemberProp, Module, ObjectLit, Prop, PropName, PropOrSpread, SimpleAssignTarget,
    Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{binding_key, BindingKey};
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};
use super::RewriteLevel;

pub struct UnDefineProperty;

impl UnDefineProperty {
    pub(crate) fn run_with_helpers(
        module: &mut Module,
        local_helpers: &LocalHelperContext,
        rewrite_level: RewriteLevel,
    ) {
        run_un_define_property(module, local_helpers, rewrite_level);
    }
}

impl VisitMut for UnDefineProperty {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_define_property(module, &local_helpers, RewriteLevel::Standard);
    }
}

fn run_un_define_property(
    module: &mut Module,
    local_helpers: &LocalHelperContext,
    rewrite_level: RewriteLevel,
) {
    let helper_map = local_helpers.helpers_of_kind(TranspilerHelperKind::DefineProperty);
    let helpers: HelperSet = helper_map.keys().cloned().collect();
    if helpers.is_empty() {
        return;
    }

    let mut rewriter = CallSiteRewriter {
        helpers: &helpers,
        rewrite_level,
        rewrote_any: false,
    };
    module.visit_mut_with(&mut rewriter);

    if rewriter.rewrote_any {
        local_helpers.remove_helpers_with_dependencies(module, helper_map);
    }
}

/// Helper declaration bindings, keyed by resolver binding identity.
type HelperSet = HashSet<BindingKey>;

struct CallSiteRewriter<'a> {
    helpers: &'a HelperSet,
    rewrite_level: RewriteLevel,
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

    fn try_rewrite_fresh_object_call(&mut self, expr: &mut Expr) -> bool {
        if self.rewrite_level < RewriteLevel::Standard {
            return false;
        }
        let Expr::Call(call) = expr else {
            return false;
        };
        let Callee::Expr(callee_expr) = &call.callee else {
            return false;
        };
        let Expr::Ident(callee_ident) = callee_expr.as_ref() else {
            return false;
        };
        if !self.helpers.contains(&binding_key(callee_ident))
            || call.args.len() != 3
            || call.args.iter().any(|arg| arg.spread.is_some())
        {
            return false;
        }
        let Expr::Object(target) = call.args[0].expr.as_ref() else {
            return false;
        };
        if !target.props.is_empty() {
            return false;
        }

        let key = call.args[1].expr.clone();
        let value = call.args[2].expr.clone();
        // Assumption: effect_free_property_key_coercion. Babel/SWC evaluate
        // the value argument before their helper coerces `key`; a computed
        // property coerces `key` first. The known producer shape uses ordinary
        // primitive property keys, but minimal mode preserves the exact order.
        *expr = Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: vec![PropOrSpread::Prop(Box::new(Prop::KeyValue(KeyValueProp {
                key: PropName::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: key,
                }),
                value,
            })))],
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

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        self.try_rewrite_fresh_object_call(expr);
    }
}
