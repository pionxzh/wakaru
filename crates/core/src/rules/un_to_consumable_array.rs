use std::collections::{HashMap, HashSet};

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayLit, BinExpr, BinaryOp, Callee, Decl, Expr, ExprOrSpread, MemberExpr, MemberProp, Module,
    ModuleItem, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, helpers_with_remaining_refs, remove_helper_declarations, BabelHelperKind,
    BindingKey,
};
use super::helper_matcher::{
    binding_key, remaining_refs_outside_var_declarators, remove_var_declarators_by_binding,
    var_declarator_binding_key,
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
            let ts_helpers = collect_ts_spread_array_helpers(module);
            if ts_helpers.is_empty() {
                return;
            }

            let mut replacer = ToConsumableArrayReplacer {
                helpers: &helpers,
                ts_spread_array_helpers: &ts_helpers,
            };
            module.visit_mut_with(&mut replacer);

            remove_unused_ts_spread_array_helpers(module, &ts_helpers);
            return;
        }

        let ts_helpers = collect_ts_spread_array_helpers(module);

        let mut replacer = ToConsumableArrayReplacer {
            helpers: &helpers,
            ts_spread_array_helpers: &ts_helpers,
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
}

struct ToConsumableArrayReplacer<'a> {
    helpers: &'a HashMap<BindingKey, BabelHelperKind>,
    ts_spread_array_helpers: &'a HashSet<BindingKey>,
}

impl VisitMut for ToConsumableArrayReplacer<'_> {
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
        if self.helpers.contains_key(&key) {
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
            return;
        }

        if self.ts_spread_array_helpers.contains(&key) {
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

fn collect_ts_spread_array_helpers(module: &Module) -> HashSet<BindingKey> {
    module
        .body
        .iter()
        .filter_map(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                return None;
            };
            if var.decls.len() != 1 {
                return None;
            }
            let decl = &var.decls[0];
            let init = decl.init.as_deref()?;
            is_ts_spread_array_helper_init(init)
                .then(|| var_declarator_binding_key(decl))
                .flatten()
        })
        .collect()
}

fn is_ts_spread_array_helper_init(expr: &Expr) -> bool {
    let expr = strip_paren_expr(expr);
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        ..
    }) = expr
    else {
        return false;
    };

    let left = strip_paren_expr(left);
    let Expr::Bin(and_bin) = left else {
        return false;
    };
    if and_bin.op != BinaryOp::LogicalAnd {
        return false;
    }

    let and_left = strip_paren_expr(and_bin.left.as_ref());
    let and_right = strip_paren_expr(and_bin.right.as_ref());

    matches!(and_left, Expr::This(_))
        && matches!(
            and_right,
            Expr::Member(MemberExpr {
                obj,
                prop: MemberProp::Ident(prop),
                ..
            }) if matches!(obj.as_ref(), Expr::This(_)) && prop.sym.as_ref() == "__spreadArray"
        )
}

fn strip_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren_expr(&paren.expr),
        _ => expr,
    }
}

fn remove_unused_ts_spread_array_helpers(module: &mut Module, helpers: &HashSet<BindingKey>) {
    let remaining = remaining_refs_outside_var_declarators(module, helpers, helpers);
    let unused: HashSet<_> = helpers.difference(&remaining).cloned().collect();
    if unused.is_empty() {
        return;
    }

    remove_var_declarators_by_binding(&mut module.body, &unused);
}
