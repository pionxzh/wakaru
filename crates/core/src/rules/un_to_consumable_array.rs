use std::collections::{HashMap, HashSet};

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{ArrayLit, Callee, Expr, ExprOrSpread, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_tslib_namespace_bindings, helpers_with_remaining_refs, is_tslib_spread_array_member,
    remove_helper_declarations, BabelHelperKind, BindingKey, LocalHelperContext, TsHelperKind,
};
use super::helper_matcher::{
    binding_key, remaining_refs_outside_var_declarators, remove_import_specifiers_by_binding,
    remove_var_declarators_by_binding,
};

/// Detects and replaces `_toConsumableArray(arr)` with `[...arr]`.
pub struct UnToConsumableArray;

impl UnToConsumableArray {
    pub(crate) fn run_with_helpers(module: &mut Module, local_helpers: &LocalHelperContext) {
        run_un_to_consumable_array(module, local_helpers);
    }
}

impl VisitMut for UnToConsumableArray {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_to_consumable_array(module, &local_helpers);
    }
}

fn run_un_to_consumable_array(module: &mut Module, local_helpers: &LocalHelperContext) {
    let helpers = local_helpers.helpers_of_kind(BabelHelperKind::ToConsumableArray);
    let ts_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::SpreadArray);
    let tslib_namespaces = collect_tslib_namespace_bindings(module);
    if helpers.is_empty() {
        if ts_helpers.is_empty() && tslib_namespaces.is_empty() {
            return;
        }

        let mut replacer = ToConsumableArrayReplacer {
            helpers: &helpers,
            ts_spread_array_helpers: &ts_helpers,
            tslib_namespaces: &tslib_namespaces,
        };
        module.visit_mut_with(&mut replacer);

        remove_unused_ts_spread_array_helpers(module, &ts_helpers);
        return;
    }

    let mut replacer = ToConsumableArrayReplacer {
        helpers: &helpers,
        ts_spread_array_helpers: &ts_helpers,
        tslib_namespaces: &tslib_namespaces,
    };
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
    remove_unused_ts_spread_array_helpers(module, &ts_helpers);
}

struct ToConsumableArrayReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    ts_spread_array_helpers: &'a HashSet<BindingKey>,
    tslib_namespaces: &'a HashSet<BindingKey>,
}

impl VisitMut for ToConsumableArrayReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };

        if let Expr::Ident(id) = callee.as_ref() {
            let key = binding_key(id);
            if self.helpers.contains_key(&key) {
                // Only transform single-argument calls
                if call.args.len() != 1 {
                    return;
                }

                // _toConsumableArray(arg) -> [...arg]
                *expr = Expr::Array(ArrayLit {
                    span: DUMMY_SP,
                    elems: vec![Some(ExprOrSpread {
                        spread: Some(DUMMY_SP),
                        expr: call.args[0].expr.clone(),
                    })],
                });
                return;
            }

            if self.ts_spread_array_helpers.contains(&key) {
                if let Some(array) = convert_ts_spread_array_call(call) {
                    *expr = Expr::Array(array);
                }
                return;
            }
        }

        if is_tslib_spread_array_member(callee, self.tslib_namespaces) {
            if let Some(array) = convert_ts_spread_array_call(call) {
                *expr = Expr::Array(array);
            }
        }
    }
}

fn convert_ts_spread_array_call(call: &swc_core::ecma::ast::CallExpr) -> Option<ArrayLit> {
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems = Vec::new();
    append_array_source(&mut elems, call.args[0].expr.as_ref(), true)?;
    append_array_source(&mut elems, call.args[1].expr.as_ref(), false)?;

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}

fn append_array_source(
    elems: &mut Vec<Option<ExprOrSpread>>,
    expr: &Expr,
    require_array_literal: bool,
) -> Option<()> {
    match expr {
        Expr::Array(array) => {
            elems.extend(array.elems.iter().cloned());
            Some(())
        }
        _ if !require_array_literal => {
            elems.push(Some(ExprOrSpread {
                spread: Some(DUMMY_SP),
                expr: Box::new(expr.clone()),
            }));
            Some(())
        }
        _ => None,
    }
}

fn remove_unused_ts_spread_array_helpers(module: &mut Module, helpers: &HashSet<BindingKey>) {
    let remaining = remaining_refs_outside_var_declarators(module, helpers, helpers);
    let unused: HashSet<_> = helpers.difference(&remaining).cloned().collect();
    if unused.is_empty() {
        return;
    }

    remove_var_declarators_by_binding(&mut module.body, &unused);
    remove_import_specifiers_by_binding(&mut module.body, &unused);
}
