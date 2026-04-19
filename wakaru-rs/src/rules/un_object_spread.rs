use std::collections::HashMap;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Callee, Expr, Module, ObjectLit, PropOrSpread, SpreadElement,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, helpers_with_remaining_refs, remove_helper_declarations, BabelHelperKind,
    BindingKey,
};

/// Detects and replaces `_extends` and `_objectSpread2` helper calls with
/// object spread syntax.
///
/// Both `_extends` and `_objectSpread2` mutate and return their first argument
/// (like Object.assign). Only transform when the first arg is an empty object
/// literal `{}`, which guarantees no mutation/identity side effects:
///   `_extends({}, obj1, obj2)` → `{ ...obj1, ...obj2 }`
///   `_objectSpread2({}, y)` → `{ ...y }`
///   `_extends(target, source)` → left as-is (mutation semantics)
///   `_objectSpread2(existing, {a: 1})` → left as-is (mutation semantics)
pub struct UnObjectSpread;

impl VisitMut for UnObjectSpread {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let all_helpers = collect_helpers(module);
        let helpers: HashMap<BindingKey, BabelHelperKind> = all_helpers
            .into_iter()
            .filter(|(_, kind)| {
                *kind == BabelHelperKind::Extends || *kind == BabelHelperKind::ObjectSpread
            })
            .collect();
        if helpers.is_empty() {
            return;
        }

        let mut replacer = SpreadReplacer { helpers: &helpers };
        module.visit_mut_with(&mut replacer);

        // Only remove declaration if no untransformed calls remain
        let remaining = helpers_with_remaining_refs(module, &helpers);
        let safe_to_remove: HashMap<BindingKey, BabelHelperKind> = helpers
            .into_iter()
            .filter(|(key, _)| !remaining.contains(key))
            .collect();
        if !safe_to_remove.is_empty() {
            remove_helper_declarations(&mut module.body, &safe_to_remove);
        }
    }
}

struct SpreadReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
}

impl VisitMut for SpreadReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else { return };
        let Expr::Ident(id) = callee.as_ref() else { return };

        let key = (id.sym.clone(), id.ctxt);
        let Some(_kind) = self.helpers.get(&key) else {
            return;
        };

        if call.args.is_empty() {
            return;
        }

        // Both _extends and _objectSpread2 mutate their first argument.
        // Only transform when the first arg is an empty object literal `{}`,
        // otherwise mutation/identity semantics are lost.
        let first_is_empty_obj = matches!(
            call.args[0].expr.as_ref(),
            Expr::Object(obj) if obj.props.is_empty()
        );
        if !first_is_empty_obj {
            return;
        }

        // Merge all arguments into a single object expression.
        // - Object literal args: flatten their properties
        // - Everything else: wrap as spread element
        let mut properties: Vec<PropOrSpread> = Vec::new();

        for arg in &call.args {
            if arg.spread.is_some() {
                properties.push(PropOrSpread::Spread(SpreadElement {
                    dot3_token: DUMMY_SP,
                    expr: arg.expr.clone(),
                }));
                continue;
            }

            match arg.expr.as_ref() {
                Expr::Object(obj) => {
                    properties.extend(obj.props.iter().cloned());
                }
                _ => {
                    properties.push(PropOrSpread::Spread(SpreadElement {
                        dot3_token: DUMMY_SP,
                        expr: arg.expr.clone(),
                    }));
                }
            }
        }

        *expr = Expr::Object(ObjectLit {
            span: DUMMY_SP,
            props: properties,
        });
    }
}
