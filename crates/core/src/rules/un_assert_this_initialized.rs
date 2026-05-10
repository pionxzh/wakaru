use std::collections::HashMap;

use swc_core::ecma::ast::{Callee, Expr, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers_of_kind, helpers_with_remaining_refs, remove_helper_declarations,
    BabelHelperKind, BindingKey,
};

/// Detects and simplifies `_assertThisInitialized(self)` helper calls.
///
/// Pattern:
/// ```js
/// function p(e) {
///     if (e === undefined) {
///         throw new ReferenceError("this hasn't been initialised...");
///     }
///     return e;
/// }
/// ```
///
/// Simplification: `p(x)` → `x` (identity function with a guard).
pub struct UnAssertThisInitialized;

impl VisitMut for UnAssertThisInitialized {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = collect_helpers_of_kind(module, BabelHelperKind::AssertThisInitialized);
        if helpers.is_empty() {
            return;
        }

        let mut replacer = AtiReplacer { helpers: &helpers };
        module.visit_mut_with(&mut replacer);

        let remaining = helpers_with_remaining_refs(module, &helpers);
        let safe: HashMap<BindingKey, BabelHelperKind> = helpers
            .into_iter()
            .filter(|(key, _)| !remaining.contains(key))
            .collect();
        if !safe.is_empty() {
            remove_helper_declarations(&mut module.body, &safe);
        }
    }
}

struct AtiReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for AtiReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = callee.as_ref() else {
            return;
        };

        let key = (id.sym.clone(), id.ctxt);
        if !self.helpers.contains_key(&key) {
            return;
        }

        if call.args.len() != 1 {
            return;
        }

        *expr = *call.args[0].expr.clone();
    }
}
