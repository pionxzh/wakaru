#[cfg(test)]
use std::cell::Cell;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, ComputedPropName, Expr, Ident, Lit, MemberExpr, Module, PropName,
    SimpleAssignTarget,
};

use super::helper_matcher::{
    binding_key, collect_refs, expr_matches_binding, member_prop_name,
    remaining_refs_outside_declarations, remaining_refs_outside_var_declarators,
    remove_fn_decls_from_body_by_binding, remove_import_specifiers_by_binding,
    remove_var_declarators_by_binding, static_member_prop_name,
};
use crate::js_names::is_likely_generated_alias;
use crate::utils::paren::strip_parens;

pub(crate) use super::helper_matcher::BindingKey;

mod collect;
mod lifecycle;
mod matchers;
mod paths;
mod ts_helpers;

pub(crate) use collect::collect_transpiler_helpers;
use collect::collect_transpiler_helpers_inner;

pub(crate) use matchers::{
    classify_inline_callable, classify_inline_helper_call, collect_maybe_array_like_bindings,
    extract_inline_sliced_to_array_call, is_call_super_fn, is_inherits_fn, is_set_prototype_of_fn,
};
use matchers::{
    detect_helper_from_fn, detect_helper_from_var_decl, generated_fn_helper_name_kind,
    is_self_redefining_typeof_fn, module_has_babel_sub_helper_signals,
};

use lifecycle::{collect_top_level_callable_ref_graph, helper_dependencies_from_ref_graph};
pub(crate) use lifecycle::{
    helpers_with_remaining_refs, remove_helper_declarations, remove_helpers_without_remaining_refs,
};
pub(crate) use paths::detect_helper_from_path;
use paths::{export_name_to_atom, named_import_is_helper};

pub(crate) use ts_helpers::{
    collect_inline_ts_helpers_deep, collect_tslib_namespace_bindings, is_tslib_path,
    is_tslib_require_expr_with_mark, is_tslib_spread_array_member, ts_expr_matches_helper_kind,
    tslib_helper_name_kind, tslib_member_helper_kind, tslib_member_ts_helper_kind,
    tslib_require_member_name, tslib_require_ts_helper_kind,
    tslib_require_ts_helper_kind_with_mark,
};
use ts_helpers::{
    collect_ts_helpers, collect_tslib_require_member_calls, detect_helper_from_tslib_require_member,
};

#[cfg(test)]
thread_local! {
    static COLLECT_TRANSPILER_HELPERS_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_collect_transpiler_helpers_call_count() {
    COLLECT_TRANSPILER_HELPERS_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(crate) fn collect_transpiler_helpers_call_count() -> usize {
    COLLECT_TRANSPILER_HELPERS_CALLS.with(Cell::get)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TranspilerHelperKind {
    InteropRequireDefault,
    InteropRequireWildcard,
    ToConsumableArray,
    Extends,
    ObjectSpread,
    SlicedToArray,
    ClassCallCheck,
    PossibleConstructorReturn,
    AssertThisInitialized,
    ObjectWithoutProperties,
    Inherits,
    CallSuper,
    AsyncToGenerator,
    TaggedTemplateLiteral,
    DefineProperty,
    CreateClass,
    Typeof,
    HelperDependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TsHelperKind {
    Awaiter,
    Generator,
    Values,
    Assign,
    Rest,
    Extends,
    ImportDefault,
    ImportStar,
    CreateBinding,
    SetModuleDefault,
    Read,
    Spread,
    SpreadArrays,
    SpreadArray,
    ClassPrivateFieldGet,
    ClassPrivateFieldSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TsHelperSource {
    Inline,
    TslibImport,
    TslibRequire,
    TslibNamespace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TsHelperInfo {
    kind: TsHelperKind,
    source: TsHelperSource,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalHelperContext {
    helpers: HashMap<BindingKey, TranspilerHelperKind>,
    ts_helpers: HashMap<BindingKey, TsHelperInfo>,
    tslib_namespaces: HashSet<BindingKey>,
    tslib_require_member_calls: HashSet<TranspilerHelperKind>,
    unresolved_mark: Option<Mark>,
    top_level_callable_ref_graph: OnceCell<HashMap<BindingKey, HashSet<BindingKey>>>,
}

impl LocalHelperContext {
    pub(crate) fn collect(module: &Module) -> Self {
        Self::collect_inner(module, None)
    }

    pub(crate) fn collect_with_mark(module: &Module, unresolved_mark: Mark) -> Self {
        Self::collect_inner(module, Some(unresolved_mark))
    }

    fn collect_inner(module: &Module, unresolved_mark: Option<Mark>) -> Self {
        let tslib_namespaces = collect_tslib_namespace_bindings(module, unresolved_mark);
        Self {
            helpers: collect_transpiler_helpers_inner(module, unresolved_mark),
            ts_helpers: collect_ts_helpers(module, &tslib_namespaces, unresolved_mark),
            tslib_namespaces,
            tslib_require_member_calls: collect_tslib_require_member_calls(module, unresolved_mark),
            unresolved_mark,
            top_level_callable_ref_graph: OnceCell::new(),
        }
    }

    pub(crate) fn helpers(&self) -> &HashMap<BindingKey, TranspilerHelperKind> {
        &self.helpers
    }

    pub(crate) fn helpers_of_kind(
        &self,
        kind: TranspilerHelperKind,
    ) -> HashMap<BindingKey, TranspilerHelperKind> {
        self.helpers
            .iter()
            .filter(|(_, helper_kind)| **helper_kind == kind)
            .map(|(key, helper_kind)| (key.clone(), *helper_kind))
            .collect()
    }

    pub(crate) fn ts_helpers_of_kind(&self, kind: TsHelperKind) -> HashSet<BindingKey> {
        self.ts_helpers
            .iter()
            .filter(|(_, helper)| helper.kind == kind)
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub(crate) fn ts_helper_kind_by_symbol(&self, local: &Atom) -> Option<TsHelperKind> {
        self.ts_helpers
            .iter()
            .find_map(|((sym, _), helper)| (sym == local).then_some(helper.kind))
    }

    pub(crate) fn remove_unused_inline_ts_helpers(
        &self,
        module: &mut Module,
        kinds: &[TsHelperKind],
    ) {
        let helper_keys: HashSet<_> = self
            .ts_helpers
            .iter()
            .filter(|(_, helper)| helper.source == TsHelperSource::Inline)
            .filter(|(_, helper)| kinds.contains(&helper.kind))
            .map(|(key, _)| key.clone())
            .collect();
        if helper_keys.is_empty() {
            return;
        }

        let remaining = remaining_refs_outside_declarations(module, &helper_keys, &helper_keys);
        let removable: HashSet<BindingKey> = helper_keys
            .into_iter()
            .filter(|key| !remaining.contains(key))
            .collect();
        if !removable.is_empty() {
            remove_var_declarators_by_binding(&mut module.body, &removable);
            remove_fn_decls_from_body_by_binding(&mut module.body, &removable);
            remove_import_specifiers_by_binding(&mut module.body, &removable);
        }
    }

    pub(crate) fn remove_unused_ts_helper_bindings(&self, module: &mut Module, kind: TsHelperKind) {
        let helper_keys = self.ts_helpers_of_kind(kind);
        if helper_keys.is_empty() {
            return;
        }

        let remaining = remaining_refs_outside_var_declarators(module, &helper_keys, &helper_keys);
        let removable: HashSet<BindingKey> = helper_keys
            .into_iter()
            .filter(|key| !remaining.contains(key))
            .collect();
        if removable.is_empty() {
            return;
        }

        remove_var_declarators_by_binding(&mut module.body, &removable);
        remove_import_specifiers_by_binding(&mut module.body, &removable);
    }

    pub(crate) fn tslib_namespaces(&self) -> &HashSet<BindingKey> {
        &self.tslib_namespaces
    }

    pub(crate) fn has_tslib_require_member_call(&self, kind: TranspilerHelperKind) -> bool {
        self.tslib_require_member_calls.contains(&kind)
    }

    pub(crate) fn helper_callee_kind(&self, callee: &Expr) -> Option<TranspilerHelperKind> {
        if let Expr::Ident(id) = callee {
            if let Some(kind) = self.helpers.get(&(id.sym.clone(), id.ctxt)) {
                return Some(*kind);
            }
        }

        tslib_member_helper_kind(callee, &self.tslib_namespaces).or_else(|| {
            tslib_require_member_name(callee, self.unresolved_mark).and_then(tslib_helper_name_kind)
        })
    }

    pub(crate) fn is_helper_callee(&self, callee: &Expr, kind: TranspilerHelperKind) -> bool {
        self.helper_callee_kind(callee) == Some(kind)
    }

    pub(crate) fn is_unresolved_or_unguarded_ident(&self, id: &Ident, name: &str) -> bool {
        is_unresolved_or_unguarded_ident(id, name, self.unresolved_mark)
    }

    pub(crate) fn helper_dependencies(
        &self,
        module: &Module,
        helpers: &HashMap<BindingKey, TranspilerHelperKind>,
    ) -> HashMap<BindingKey, TranspilerHelperKind> {
        let ref_graph = self
            .top_level_callable_ref_graph
            .get_or_init(|| collect_top_level_callable_ref_graph(module));
        helper_dependencies_from_ref_graph(ref_graph, helpers)
    }

    pub(crate) fn helper_cleanup_candidates_with_dependencies(
        &self,
        module: &Module,
        root_helpers: HashMap<BindingKey, TranspilerHelperKind>,
    ) -> HashMap<BindingKey, TranspilerHelperKind> {
        let remaining_roots = helpers_with_remaining_refs(module, &root_helpers);
        let removable_roots: HashMap<BindingKey, TranspilerHelperKind> = root_helpers
            .into_iter()
            .filter(|(key, _)| !remaining_roots.contains(key))
            .collect();
        if removable_roots.is_empty() {
            return HashMap::new();
        }

        let helper_dependencies = self.helper_dependencies(module, &removable_roots);
        removable_roots
            .into_iter()
            .chain(helper_dependencies)
            .collect()
    }

    pub(crate) fn remove_helpers_with_dependencies(
        &self,
        module: &mut Module,
        root_helpers: HashMap<BindingKey, TranspilerHelperKind>,
    ) {
        let removable_helpers =
            self.helper_cleanup_candidates_with_dependencies(module, root_helpers);
        remove_helpers_without_remaining_refs(module, removable_helpers);
    }
}

// ---------------------------------------------------------------------------
// interopRequireDefault body-shape matchers
// ---------------------------------------------------------------------------

fn is_object_member(member: &MemberExpr, prop: &str) -> bool {
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Object" && member_prop_name(&member.prop, prop)
}

fn is_symbol_member(member: &MemberExpr, prop: &str) -> bool {
    let Expr::Ident(obj) = member.obj.as_ref() else {
        return false;
    };
    obj.sym.as_ref() == "Symbol" && member_prop_name(&member.prop, prop)
}

fn is_member_call(expr: &Expr, prop: &str) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    member_prop_name(&member.prop, prop)
}

fn prop_name_as_str(name: &PropName) -> Option<&str> {
    match name {
        PropName::Ident(id) => Some(id.sym.as_ref()),
        PropName::Str(s) => s.value.as_str(),
        PropName::Computed(ComputedPropName { expr, .. }) => match expr.as_ref() {
            Expr::Lit(Lit::Str(s)) => s.value.as_str(),
            _ => None,
        },
        PropName::Num(_) | PropName::BigInt(_) => None,
    }
}

fn is_unresolved_or_unguarded_ident(id: &Ident, name: &str, unresolved_mark: Option<Mark>) -> bool {
    id.sym.as_ref() == name
        && unresolved_mark.is_none_or(|unresolved_mark| id.ctxt.outer() == unresolved_mark)
}

// ---------------------------------------------------------------------------
// interopRequireWildcard body-shape matcher
//
// The wildcard helper is more complex and varies across versions. We use a
// looser match: 1-2 params, body references `__esModule`, and contains
// property-copying logic (for-in or Object.keys/getOwnPropertyDescriptor).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// toConsumableArray body-shape matcher
//
// Babel 7+: function(arr) { return f(arr) || g(arr) || h(arr) || k(); }
//   where the sub-helpers reference Array.isArray / Array.from
// Babel 6:  function(arr) { if (Array.isArray(arr)) { ... } else { return Array.from(arr); } }
//
// Key signal: 1 param, body references both Array.isArray and Array.from.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// extends body-shape matcher
//
// function _extends() {
//   _extends = Object.assign || function(target) { ... for-in ... };
//   return _extends.apply(this, arguments);
// }
// Or minified: function() { return n = Object.assign || ..., n.apply(this, arguments); }
//
// Key signal: 0 params, references Object.assign, has .apply(this, arguments).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// classCallCheck body-shape matcher
//
// function _classCallCheck(instance, Constructor) {
//   if (!(instance instanceof Constructor)) {
//     throw new TypeError("Cannot call a class as a function");
//   }
// }
//
// Key signal: 2 params, single if statement with !(param1 instanceof param2),
// throws TypeError.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// possibleConstructorReturn body-shape matcher
//
// function _possibleConstructorReturn(self, call) {
//   if (!self) throw new ReferenceError("this hasn't been initialised...");
//   if (!call || typeof call != "object" && typeof call != "function") return self;
//   return call;
// }
//
// Key signal: 2 params, first stmt throws ReferenceError on !param1,
// second stmt tests typeof param2, returns param1 or param2.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// _assertThisInitialized
//
// function p(e) {
//     if (e === undefined) {
//         throw new ReferenceError("this hasn't been initialised - super() hasn't been called");
//     }
//     return e;
// }
//
// Key signal: 1 param, throws ReferenceError with the Babel-specific message,
// returns param. We match on the error message text to avoid false positives.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// _objectWithoutProperties / _objectWithoutPropertiesLoose
//
// Both variants take (source, excluded_keys_array) and return a new object
// with the excluded keys filtered out. Two body shapes exist:
//
// Variant A (for-in + hasOwnProperty):
//   var t = {}; for (var k in s) { excl.indexOf(k)...; t[k] = s[k]; } return t;
//
// Variant B (Object.keys + for loop):
//   if (s == null) return {};
//   var t = {}; var keys = Object.keys(s);
//   for (i = 0; i < keys.length; i++) { excl.indexOf(...)...; t[k] = s[k]; }
//   return t;
//
// Key signal: 2 params, body uses `.indexOf` on the second param,
// initializes an empty object, and returns it.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// _extends polyfill form: Object.assign || function(target) { for-in arguments }
//
// var _extends = Object.assign || function(target) {
//   for (var i = 1; i < arguments.length; i++) {
//     var source = arguments[i];
//     for (var key in source) {
//       if (Object.prototype.hasOwnProperty.call(source, key))
//         target[key] = source[key];
//     }
//   }
//   return target;
// };
//
// Key signal: 1 param, references `arguments`, loops with for-in, returns param.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// objectSpread / objectSpread2 body-shape matcher
//
// function _objectSpread2(target) {
//   for (var i = 1; i < arguments.length; i++) { ... Object.defineProperty ... }
//   return target;
// }
//
// Key signal: 1 param, references `arguments`, contains Object.defineProperty
// or Object.getOwnPropertyDescriptor/getOwnPropertyDescriptors, returns param.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// slicedToArray body-shape matcher
//
// Babel 7+: function(arr, i) { return f(arr) || g(arr, i) || h(arr, i) || k(); }
// Babel 6:  function(arr, i) { if (Array.isArray(arr)) { ... } else if (Symbol.iterator in ...) { ... } ... }
//
// Key signal: 2 params, body references Symbol.iterator or is a logical-OR
// chain of sub-helper calls with 2 params.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Shared body scanning infrastructure
// ---------------------------------------------------------------------------

// ============================================================
// _inherits helper detection
// ============================================================

// ============================================================
// _callSuper helper detection
// ============================================================

// ---------------------------------------------------------------------------
// maybeArrayLike body-shape matcher
//
// function _maybeArrayLike(orElse, arr, i) {
//   if (arr && !Array.isArray(arr) && typeof arr.length === "number") {
//     var len = arr.length;
//     return _arrayLikeToArray(arr, i !== void 0 && i < len ? len : len);
//   }
//   return orElse(arr, i);
// }
//
// Key signals: 3 params, body has !Array.isArray + typeof .length === "number",
// and a `return param0(param1, ...)` that delegates to the first parameter.
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
