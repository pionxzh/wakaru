use std::collections::{HashMap, HashSet};

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrayLit, BinExpr, BinaryOp, Callee, Decl, Expr, ExprOrSpread, ImportSpecifier, MemberExpr,
    MemberProp, Module, ModuleDecl, ModuleItem, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::babel_helper_utils::{
    collect_helpers, collect_tslib_namespace_bindings, helpers_with_remaining_refs, is_tslib_path,
    is_tslib_spread_array_member, remove_helper_declarations, tslib_require_member_name,
    BabelHelperKind, BindingKey,
};
use super::helper_matcher::{
    binding_key, remaining_refs_outside_var_declarators, remove_import_specifiers_by_binding,
    remove_var_declarators_by_binding, var_declarator_binding_key,
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
        let tslib_namespaces = collect_tslib_namespace_bindings(module);
        if helpers.is_empty() {
            let ts_helpers = collect_ts_spread_array_helpers(module);
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

        let ts_helpers = collect_ts_spread_array_helpers(module);

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

fn collect_ts_spread_array_helpers(module: &Module) -> HashSet<BindingKey> {
    module
        .body
        .iter()
        .flat_map(|item| {
            let mut helpers = Vec::new();
            match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) if var.decls.len() == 1 => {
                    let decl = &var.decls[0];
                    if let Some(init) = decl.init.as_deref() {
                        let is_helper = is_ts_spread_array_helper_init(init)
                            || tslib_require_member_name(init) == Some("__spreadArray");
                        if is_helper {
                            if let Some(key) = var_declarator_binding_key(decl) {
                                helpers.push(key);
                            }
                        }
                    }
                }
                ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                    if !import.type_only
                        && is_tslib_path(import.src.value.as_str().unwrap_or("")) =>
                {
                    for specifier in &import.specifiers {
                        let ImportSpecifier::Named(named) = specifier else {
                            continue;
                        };
                        let imported = named
                            .imported
                            .as_ref()
                            .map(|name| match name {
                                swc_core::ecma::ast::ModuleExportName::Ident(id) => id.sym.as_ref(),
                                swc_core::ecma::ast::ModuleExportName::Str(s) => {
                                    s.value.as_str().unwrap_or("")
                                }
                            })
                            .unwrap_or(named.local.sym.as_ref());
                        if imported == "__spreadArray" {
                            helpers.push(binding_key(&named.local));
                        }
                    }
                }
                _ => {}
            }
            helpers
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
    remove_import_specifiers_by_binding(&mut module.body, &unused);
}
