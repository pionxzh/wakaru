use std::collections::HashMap;

use swc_core::ecma::ast::{Callee, Expr, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers_of_kind, helpers_with_remaining_refs, remove_helper_declarations,
    BabelHelperKind, BindingKey,
};

/// Detects and simplifies `_possibleConstructorReturn(self, call)` helper calls.
///
/// Pattern:
/// ```js
/// function d(self, call) {
///     if (!self) throw new ReferenceError("this hasn't been initialised...");
///     if (!call || typeof call != "object" && typeof call != "function") return self;
///     return call;
/// }
/// ```
///
/// Simplification: `d(a, b)` → `b` (returns the second argument).
/// The helper only returns `self` when `call` is a primitive — which never happens
/// when wrapping a super() constructor call in ES6 class semantics.
pub struct UnPossibleConstructorReturn;

impl VisitMut for UnPossibleConstructorReturn {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let helpers = collect_helpers_of_kind(module, BabelHelperKind::PossibleConstructorReturn);
        if helpers.is_empty() {
            return;
        }

        let mut replacer = PcrReplacer { helpers: &helpers };
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

struct PcrReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for PcrReplacer<'_> {
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

        // Must have exactly 2 arguments: (self, call)
        if call.args.len() != 2 {
            return;
        }

        // Replace with the second argument (the super constructor return value)
        *expr = *call.args[1].expr.clone();
    }
}
