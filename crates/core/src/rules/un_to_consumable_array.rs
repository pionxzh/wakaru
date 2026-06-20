use std::collections::{HashMap, HashSet};

use swc_core::common::util::take::Take;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{ArrayLit, Callee, Expr, ExprOrSpread, Module};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};

use super::cross_module_helper_refs::{
    collect_cross_module_ts_helper_refs, cross_module_ts_member_helper,
};
use super::helper_matcher::binding_key;
use super::transpiler_helper_utils::{
    collect_maybe_array_like_bindings, is_tslib_spread_array_member, ts_expr_matches_helper_kind,
    tslib_member_ts_helper_kind, tslib_require_ts_helper_kind, BindingKey, LocalHelperContext,
    TranspilerHelperKind, TsHelperKind,
};

/// Detects and replaces `_toConsumableArray(arr)` with `[...arr]`.
pub struct UnToConsumableArray<'a> {
    module_facts: Option<&'a ModuleFactsMap>,
    unresolved_mark: Option<Mark>,
}

impl UnToConsumableArray<'_> {
    pub fn new() -> Self {
        Self {
            module_facts: None,
            unresolved_mark: None,
        }
    }

    pub fn new_with_mark(unresolved_mark: Mark) -> Self {
        Self {
            module_facts: None,
            unresolved_mark: Some(unresolved_mark),
        }
    }
}

impl<'a> UnToConsumableArray<'a> {
    pub fn new_with_facts(module_facts: &'a ModuleFactsMap) -> Self {
        Self {
            module_facts: Some(module_facts),
            unresolved_mark: None,
        }
    }

    pub(crate) fn run_with_helpers(
        module: &mut Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
    ) {
        run_un_to_consumable_array(module, Some(unresolved_mark), local_helpers, module_facts);
    }
}

impl Default for UnToConsumableArray<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl VisitMut for UnToConsumableArray<'_> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        run_un_to_consumable_array(
            module,
            self.unresolved_mark,
            &local_helpers,
            self.module_facts,
        );
    }
}

fn run_un_to_consumable_array(
    module: &mut Module,
    unresolved_mark: Option<Mark>,
    local_helpers: &LocalHelperContext,
    module_facts: Option<&ModuleFactsMap>,
) {
    let helpers = local_helpers.helpers_of_kind(TranspilerHelperKind::ToConsumableArray);
    let ts_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::SpreadArray);
    let mut ts_legacy_array_spread_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::Spread);
    ts_legacy_array_spread_helpers
        .extend(local_helpers.ts_helpers_of_kind(TsHelperKind::SpreadArrays));
    let ts_read_helpers = local_helpers.ts_helpers_of_kind(TsHelperKind::Read);
    let cross_module_ts_helpers = module_facts
        .map(|facts| {
            collect_cross_module_ts_helper_refs(module, facts, TypeScriptHelperKind::SpreadArray)
        })
        .unwrap_or_default();
    let mut cross_module_ts_legacy_array_spread_helpers = module_facts
        .map(|facts| {
            collect_cross_module_ts_helper_refs(module, facts, TypeScriptHelperKind::Spread)
        })
        .unwrap_or_default();
    if let Some(facts) = module_facts {
        cross_module_ts_legacy_array_spread_helpers.extend(collect_cross_module_ts_helper_refs(
            module,
            facts,
            TypeScriptHelperKind::SpreadArrays,
        ));
    }
    let cross_module_ts_read_helpers = module_facts
        .map(|facts| collect_cross_module_ts_helper_refs(module, facts, TypeScriptHelperKind::Read))
        .unwrap_or_default();
    let tslib_namespaces = local_helpers.tslib_namespaces();
    let maybe_array_like = collect_maybe_array_like_bindings(module);
    let has_inline_legacy_array_spread = has_inline_legacy_array_spread_call(module);
    let has_direct_tslib_array_spread = has_direct_tslib_array_spread_call(module, unresolved_mark);
    if helpers.is_empty() {
        if ts_helpers.is_empty()
            && ts_legacy_array_spread_helpers.is_empty()
            && cross_module_ts_helpers.direct.is_empty()
            && cross_module_ts_helpers.namespaces.is_empty()
            && cross_module_ts_legacy_array_spread_helpers
                .direct
                .is_empty()
            && cross_module_ts_legacy_array_spread_helpers
                .namespaces
                .is_empty()
            && tslib_namespaces.is_empty()
            && !has_direct_tslib_array_spread
            && !has_inline_legacy_array_spread
        {
            return;
        }

        let mut replacer = ToConsumableArrayReplacer {
            helpers: &helpers,
            maybe_array_like: &maybe_array_like,
            ts_spread_array_helpers: &ts_helpers,
            ts_legacy_array_spread_helpers: &ts_legacy_array_spread_helpers,
            cross_module_ts_spread_array_helpers: &cross_module_ts_helpers.direct,
            cross_module_ts_spread_array_namespaces: &cross_module_ts_helpers.namespaces,
            cross_module_ts_legacy_array_spread_helpers:
                &cross_module_ts_legacy_array_spread_helpers.direct,
            cross_module_ts_legacy_array_spread_namespaces:
                &cross_module_ts_legacy_array_spread_helpers.namespaces,
            ts_read_helpers: &ts_read_helpers,
            cross_module_ts_read_helpers: &cross_module_ts_read_helpers.direct,
            cross_module_ts_read_namespaces: &cross_module_ts_read_helpers.namespaces,
            tslib_namespaces,
            unresolved_mark,
        };
        module.visit_mut_with(&mut replacer);

        local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArray);
        local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::Spread);
        local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArrays);
        return;
    }

    let mut replacer = ToConsumableArrayReplacer {
        helpers: &helpers,
        maybe_array_like: &maybe_array_like,
        ts_spread_array_helpers: &ts_helpers,
        ts_legacy_array_spread_helpers: &ts_legacy_array_spread_helpers,
        cross_module_ts_spread_array_helpers: &cross_module_ts_helpers.direct,
        cross_module_ts_spread_array_namespaces: &cross_module_ts_helpers.namespaces,
        cross_module_ts_legacy_array_spread_helpers: &cross_module_ts_legacy_array_spread_helpers
            .direct,
        cross_module_ts_legacy_array_spread_namespaces:
            &cross_module_ts_legacy_array_spread_helpers.namespaces,
        ts_read_helpers: &ts_read_helpers,
        cross_module_ts_read_helpers: &cross_module_ts_read_helpers.direct,
        cross_module_ts_read_namespaces: &cross_module_ts_read_helpers.namespaces,
        tslib_namespaces,
        unresolved_mark,
    };
    module.visit_mut_with(&mut replacer);

    // Remove the helper and any inline sub-helpers it transitively pulled in
    // (`_arrayWithoutHoles`, `_iterableToArray`, …) once the call sites are gone,
    // so the non-external (inlined) lowering does not leave dead declarations.
    local_helpers.remove_helpers_with_dependencies(module, helpers);
    local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArray);
    local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::Spread);
    local_helpers.remove_unused_ts_helper_bindings(module, TsHelperKind::SpreadArrays);
}

struct ToConsumableArrayReplacer<'a> {
    helpers: &'a HashMap<BindingKey, TranspilerHelperKind>,
    maybe_array_like: &'a HashSet<BindingKey>,
    ts_spread_array_helpers: &'a HashSet<BindingKey>,
    ts_legacy_array_spread_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_spread_array_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_spread_array_namespaces: &'a HashMap<BindingKey, HashSet<String>>,
    cross_module_ts_legacy_array_spread_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_legacy_array_spread_namespaces: &'a HashMap<BindingKey, HashSet<String>>,
    ts_read_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_read_helpers: &'a HashSet<BindingKey>,
    cross_module_ts_read_namespaces: &'a HashMap<BindingKey, HashSet<String>>,
    tslib_namespaces: &'a HashSet<BindingKey>,
    unresolved_mark: Option<Mark>,
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
                if call.args.len() != 1 {
                    return;
                }
                *expr = Expr::Array(ArrayLit {
                    span: DUMMY_SP,
                    elems: vec![Some(ExprOrSpread {
                        spread: Some(DUMMY_SP),
                        expr: call.args[0].expr.take(),
                    })],
                });
                return;
            }

            // _maybeArrayLike(_toConsumableArray, arg) -> [...arg]
            if self.maybe_array_like.contains(&key)
                && call.args.len() == 2
                && call.args.iter().all(|a| a.spread.is_none())
            {
                if let Expr::Ident(inner) = call.args[0].expr.as_ref() {
                    if self.helpers.contains_key(&binding_key(inner)) {
                        *expr = Expr::Array(ArrayLit {
                            span: DUMMY_SP,
                            elems: vec![Some(ExprOrSpread {
                                spread: Some(DUMMY_SP),
                                expr: call.args[1].expr.take(),
                            })],
                        });
                        return;
                    }
                }
            }

            if self.ts_spread_array_helpers.contains(&key)
                || self.cross_module_ts_spread_array_helpers.contains(&key)
            {
                if let Some(array) = convert_ts_spread_array_call(call, self) {
                    *expr = Expr::Array(array);
                }
                return;
            }

            if self.ts_legacy_array_spread_helpers.contains(&key)
                || self
                    .cross_module_ts_legacy_array_spread_helpers
                    .contains(&key)
            {
                if let Some(array) = convert_ts_legacy_spread_call(call) {
                    *expr = Expr::Array(array);
                }
                return;
            }
        }

        if is_tslib_spread_array_member(callee, self.tslib_namespaces) {
            if let Some(array) = convert_ts_spread_array_call(call, self) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if tslib_require_ts_helper_kind(callee, self.unresolved_mark)
            == Some(TsHelperKind::SpreadArray)
        {
            if let Some(array) = convert_ts_spread_array_call(call, self) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if matches!(
            tslib_member_ts_helper_kind(callee, self.tslib_namespaces),
            Some(TsHelperKind::Spread | TsHelperKind::SpreadArrays)
        ) {
            if let Some(array) = convert_ts_legacy_spread_call(call) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if matches!(
            tslib_require_ts_helper_kind(callee, self.unresolved_mark),
            Some(TsHelperKind::Spread | TsHelperKind::SpreadArrays)
        ) {
            if let Some(array) = convert_ts_legacy_spread_call(call) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if cross_module_ts_member_helper(
            callee,
            self.cross_module_ts_legacy_array_spread_namespaces,
        ) {
            if let Some(array) = convert_ts_legacy_spread_call(call) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if ts_expr_matches_helper_kind(callee, TsHelperKind::Spread)
            || ts_expr_matches_helper_kind(callee, TsHelperKind::SpreadArrays)
        {
            if let Some(array) = convert_ts_legacy_spread_call(call) {
                *expr = Expr::Array(array);
            }
            return;
        }

        if cross_module_ts_member_helper(callee, self.cross_module_ts_spread_array_namespaces) {
            if let Some(array) = convert_ts_spread_array_call(call, self) {
                *expr = Expr::Array(array);
            }
        }
    }
}

fn has_inline_legacy_array_spread_call(module: &Module) -> bool {
    struct Finder {
        found: bool,
    }

    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.found {
                return;
            }
            let Callee::Expr(callee) = &call.callee else {
                return;
            };
            if ts_expr_matches_helper_kind(callee, TsHelperKind::Spread)
                || ts_expr_matches_helper_kind(callee, TsHelperKind::SpreadArrays)
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    use swc_core::ecma::visit::VisitWith;

    let mut finder = Finder { found: false };
    module.visit_with(&mut finder);
    finder.found
}

fn has_direct_tslib_array_spread_call(module: &Module, unresolved_mark: Option<Mark>) -> bool {
    struct Finder {
        unresolved_mark: Option<Mark>,
        found: bool,
    }

    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if self.found {
                return;
            }
            let Callee::Expr(callee) = &call.callee else {
                return;
            };
            if matches!(
                tslib_require_ts_helper_kind(callee, self.unresolved_mark),
                Some(TsHelperKind::SpreadArray | TsHelperKind::Spread | TsHelperKind::SpreadArrays)
            ) {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    use swc_core::ecma::visit::VisitWith;

    let mut finder = Finder {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

fn convert_ts_spread_array_call(
    call: &swc_core::ecma::ast::CallExpr,
    helpers: &ToConsumableArrayReplacer,
) -> Option<ArrayLit> {
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems = Vec::new();
    append_array_source(&mut elems, call.args[0].expr.as_ref(), true)?;
    let from = unwrap_ts_read_arg(call.args[1].expr.as_ref(), helpers)
        .unwrap_or_else(|| call.args[1].expr.clone());
    append_array_source(&mut elems, from.as_ref(), false)?;

    Some(ArrayLit {
        span: DUMMY_SP,
        elems,
    })
}

fn unwrap_ts_read_arg(expr: &Expr, helpers: &ToConsumableArrayReplacer) -> Option<Box<Expr>> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.len() != 1 || call.args[0].spread.is_some() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };

    if is_ts_read_callee(callee, helpers) {
        return Some(call.args[0].expr.clone());
    }

    None
}

fn is_ts_read_callee(callee: &Expr, helpers: &ToConsumableArrayReplacer) -> bool {
    if let Expr::Ident(id) = callee {
        let key = binding_key(id);
        if helpers.ts_read_helpers.contains(&key)
            || helpers.cross_module_ts_read_helpers.contains(&key)
        {
            return true;
        }
    }

    matches!(
        tslib_member_ts_helper_kind(callee, helpers.tslib_namespaces),
        Some(TsHelperKind::Read)
    ) || tslib_require_ts_helper_kind(callee, helpers.unresolved_mark) == Some(TsHelperKind::Read)
        || ts_expr_matches_helper_kind(callee, TsHelperKind::Read)
        || cross_module_ts_member_helper(callee, helpers.cross_module_ts_read_namespaces)
}

fn convert_ts_legacy_spread_call(call: &swc_core::ecma::ast::CallExpr) -> Option<ArrayLit> {
    if call.args.is_empty() || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let mut elems = Vec::new();
    for arg in &call.args {
        append_array_source(&mut elems, arg.expr.as_ref(), false)?;
    }

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
