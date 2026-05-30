use std::collections::HashSet;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{Callee, Expr, Module, UnaryExpr, UnaryOp};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{BabelHelperKind, LocalHelperContext};
use super::helper_matcher::{
    binding_key, remaining_refs_outside_var_declarators, remove_var_declarators_by_binding,
    BindingKey,
};

/// Detects and simplifies Babel's `_typeof` polyfill.
///
/// Pattern:
/// ```js
/// var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
///     ? function(e) { return typeof e; }
///     : function(e) { /* Symbol polyfill */ return typeof e; };
/// ```
///
/// All calls `_typeof(expr)` are replaced with `typeof expr`, and the
/// polyfill declaration is removed.
pub struct UnTypeofPolyfill;

impl UnTypeofPolyfill {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_typeof_polyfill(module, local_helpers);
    }
}

impl VisitMut for UnTypeofPolyfill {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_typeof_polyfill(module, &local_helpers);
    }
}

fn run_un_typeof_polyfill(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers
        .helpers_of_kind(BabelHelperKind::Typeof)
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    if helpers.is_empty() {
        return;
    }

    let mut replacer = TypeofReplacer { helpers: &helpers };
    module.visit_mut_with(&mut replacer);

    // Remove declarations if no remaining references
    let remaining = remaining_refs_outside_var_declarators(module, &helpers, &helpers);
    let safe_to_remove: HashSet<BindingKey> = helpers.difference(&remaining).cloned().collect();
    if !safe_to_remove.is_empty() {
        remove_var_declarators_by_binding(&mut module.body, &safe_to_remove);
    }
}

// ---------------------------------------------------------------------------
// Replacement: _typeof(expr) → typeof expr
// ---------------------------------------------------------------------------

struct TypeofReplacer<'a> {
    helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for TypeofReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        let key = binding_key(id);
        if !self.helpers.contains(&key) {
            return;
        }

        // Must be a single-arg call: _typeof(expr)
        if call.args.len() != 1 || call.args[0].spread.is_some() {
            return;
        }

        *expr = Expr::Unary(UnaryExpr {
            span: DUMMY_SP,
            op: UnaryOp::TypeOf,
            arg: call.args[0].expr.clone(),
        });
    }
}
