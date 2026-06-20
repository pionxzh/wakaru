use std::collections::HashMap;

use swc_core::common::util::take::Take;
use swc_core::ecma::ast::{Callee, Expr, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::transpiler_helper_utils::{
    remove_helpers_without_remaining_refs, BindingKey, LocalHelperContext, TranspilerHelperKind,
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
/// Simplification: `p(this)` → `this`.
///
/// The helper is not a general identity function: `p(x)` must still throw when
/// `x` is `undefined`. Rewriting is only safe for direct `this`, because reading
/// `this` before `super()` already throws before the helper call can run.
pub struct UnAssertThisInitialized;

impl UnAssertThisInitialized {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_assert_this_initialized(module, local_helpers);
    }
}

impl VisitMut for UnAssertThisInitialized {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_assert_this_initialized(module, &local_helpers);
    }
}

fn run_un_assert_this_initialized(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::AssertThisInitialized);
    if helpers.is_empty() {
        return;
    }

    let mut replacer = AtiReplacer { helpers: &helpers };
    module.visit_mut_with(&mut replacer);

    remove_helpers_without_remaining_refs(module, helpers);
}

struct AtiReplacer<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
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

        if matches!(call.args[0].expr.as_ref(), Expr::This(_)) {
            *expr = *call.args[0].expr.take();
        }
    }
}
