use std::collections::HashMap;

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    Callee, Expr, Module, ObjectLit, PropOrSpread, SpreadElement,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, remove_helper_declarations, BabelHelperKind, BindingKey,
};

/// Detects and replaces `_extends` and `_objectSpread2` helper calls with
/// object spread syntax.
///
/// Transforms:
///   `_extends({}, obj1, obj2)` → `{ ...obj1, ...obj2 }`
///   `_objectSpread2({ x }, y)` → `{ x, ...y }`
///   `_objectSpread2({ x: z }, { y: 'bar' })` → `{ x: z, y: 'bar' }`
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

        remove_helper_declarations(&mut module.body, &helpers);
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

        if !self.helpers.contains_key(&(id.sym.clone(), id.ctxt)) {
            return;
        }

        if call.args.is_empty() {
            return;
        }

        // Merge all arguments into a single object expression.
        // - Object literal args: flatten their properties
        // - Everything else: wrap as spread element
        let mut properties: Vec<PropOrSpread> = Vec::new();

        for arg in &call.args {
            if arg.spread.is_some() {
                // Already a spread argument — keep as spread
                properties.push(PropOrSpread::Spread(SpreadElement {
                    dot3_token: DUMMY_SP,
                    expr: arg.expr.clone(),
                }));
                continue;
            }

            match arg.expr.as_ref() {
                Expr::Object(obj) => {
                    // Flatten object properties directly
                    properties.extend(obj.props.iter().cloned());
                }
                _ => {
                    // Wrap non-object args as spread: ...arg
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
