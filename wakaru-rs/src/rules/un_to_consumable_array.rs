use std::collections::HashMap;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{ArrayLit, Callee, Expr, ExprOrSpread, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, remove_helper_declarations, BabelHelperKind, BindingKey,
};

/// Detects and replaces `_toConsumableArray(arr)` with `[...arr]`.
pub struct UnToConsumableArray;

impl VisitMut for UnToConsumableArray {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| *kind == BabelHelperKind::ToConsumableArray)
            .collect();
        if helpers.is_empty() {
            return;
        }

        let mut replacer = ToConsumableArrayReplacer { helpers: &helpers };
        module.visit_mut_with(&mut replacer);

        remove_helper_declarations(&mut module.body, &helpers);
    }
}

struct ToConsumableArrayReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for ToConsumableArrayReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else { return };
        let Expr::Ident(id) = callee.as_ref() else { return };

        if !self.helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
            return;
        }

        // Only transform single-argument calls
        if call.args.len() != 1 {
            return;
        }

        // _toConsumableArray(arg) → [...arg]
        *expr = Expr::Array(ArrayLit {
            span: DUMMY_SP,
            elems: vec![Some(ExprOrSpread {
                spread: Some(DUMMY_SP),
                expr: call.args[0].expr.clone(),
            })],
        });
    }
}
