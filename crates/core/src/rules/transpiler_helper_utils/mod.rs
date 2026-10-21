use crate::collections::{HashMap, HashSet};
#[cfg(test)]
use std::cell::Cell;
use std::cell::OnceCell;

use swc_core::atoms::Atom;
use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, ComputedPropName, Expr, Ident, Lit, MemberExpr, Module, PropName,
    SimpleAssignTarget,
};

use super::helper_matcher::{
    binding_key, collect_refs, expr_matches_binding, member_prop_name,
    remaining_refs_outside_declarations, remaining_refs_outside_var_declarators,
    removable_without_remaining_refs, removable_without_remaining_var_declarator_refs,
    remove_fn_decls_from_body_by_binding, remove_import_specifiers_by_binding,
    remove_var_declarators_by_binding, static_member_prop_name,
};
use crate::js_names::is_likely_generated_alias;
use crate::provider_helpers::ProviderHelpers;
use crate::utils::paren::strip_parens;

pub(crate) use super::helper_matcher::BindingKey;

mod collect;
mod lifecycle;
mod matchers;
mod paths;
mod ts_helpers;
mod ts_registrar;

pub(crate) use collect::collect_transpiler_helpers;
use collect::{collect_swc_member_helpers, collect_transpiler_helpers_inner};

pub(crate) use matchers::{
    classify_inline_callable, classify_inline_helper_call, classify_interop_helper_expr,
    classify_interop_helper_function, collect_maybe_array_like_bindings,
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
    ts_import_star_function_matches, tslib_helper_name_kind, tslib_member_helper_kind,
    tslib_member_ts_helper_kind, tslib_require_member_name, tslib_require_ts_helper_kind,
    tslib_require_ts_helper_kind_with_mark,
};
use ts_helpers::{
    collect_ts_helpers, collect_tslib_require_member_calls, detect_helper_from_tslib_require_member,
};

pub(crate) use ts_registrar::{
    collect_ts_helper_export_registrars, is_ts_helper_export_registration_call,
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
    AsyncIterator,
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
    AsyncValues,
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
    swc_member_helpers: HashMap<BindingKey, TranspilerHelperKind>,
    swc_member_helper_namespaces: HashMap<BindingKey, TranspilerHelperKind>,
    ts_helpers: HashMap<BindingKey, TsHelperInfo>,
    tslib_namespaces: HashSet<BindingKey>,
    tslib_require_member_calls: HashSet<TranspilerHelperKind>,
    /// Helpers of the modules of an unpack that this module requires, by
    /// the source it names each with: the barrier-0 plan's proven exports
    /// ([`Self::with_bundled_helpers`]).
    bundled: HashMap<String, ProviderHelpers>,
    /// Top-level `x = require(source)` bindings of those sources, never
    /// assigned: each reaches the helpers its provider exports, as a
    /// `require("tslib")` binding reaches tslib's.
    bundled_namespaces: HashMap<BindingKey, String>,
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
        let (swc_member_helpers, swc_member_helper_namespaces) =
            collect_swc_member_helpers(module, unresolved_mark);
        let mut helpers = collect_transpiler_helpers_inner(module, unresolved_mark);
        // A modern SWC runtime require returns a namespace, not its `_` export.
        // Async consumers use the separate member facts for these bindings.
        helpers.retain(|key, kind| {
            *kind != TranspilerHelperKind::AsyncToGenerator
                || !swc_member_helper_namespaces.contains_key(key)
        });
        Self {
            helpers,
            swc_member_helpers,
            swc_member_helper_namespaces,
            ts_helpers: collect_ts_helpers(module, &tslib_namespaces, unresolved_mark),
            tslib_namespaces,
            tslib_require_member_calls: collect_tslib_require_member_calls(module, unresolved_mark),
            bundled: HashMap::default(),
            bundled_namespaces: HashMap::default(),
            unresolved_mark,
            top_level_callable_ref_graph: OnceCell::new(),
        }
    }

    /// Adds the helpers that other modules of an unpack export, as the
    /// barrier-0 plan proved them, keyed by the source this module requires
    /// each provider with. A provider is trusted per export and kind, never
    /// by its name:
    ///
    /// - `var ns = require("./m.js")` makes `ns.name` a helper when the
    ///   provider's `name` export is one, like a `require("tslib")` binding;
    /// - `require("./m.js").name(…)` is the inlined form of the same call,
    ///   like `require("tslib").__importDefault(…)`;
    /// - a provider whose required value is the helper
    ///   (`module.exports = helper`) makes `var h = require("./m.js")` a
    ///   helper binding, as a helper runtime path does, and
    ///   `require("./m.js")(…)` a helper call; `var h =
    ///   require("./m.js").default` and `h.default(…)` read its `default`
    ///   mirror when the provider has one.
    ///
    /// A binding assigned anywhere (an assignment, an update, a loop head)
    /// is not trusted. A second `var` declarator of the same name is not
    /// counted as an assignment, as for a local helper binding: a minifier
    /// that merges variables stores a helper call's result in the helper's
    /// own binding (`var n = require("./m.js").default, n = n(require(…))`),
    /// and the call still reads the helper.
    pub(crate) fn with_bundled_helpers(
        mut self,
        module: &Module,
        bundled: HashMap<String, ProviderHelpers>,
    ) -> Self {
        if bundled.is_empty() {
            return self;
        }
        for binding in top_level_required_bindings(module, self.unresolved_mark) {
            let Some(helpers) = bundled.get(binding.source) else {
                continue;
            };
            match binding.member {
                None => {
                    if let Some(kind) = helpers.value() {
                        self.helpers.insert(binding.key.clone(), kind);
                    }
                    self.bundled_namespaces
                        .insert(binding.key, binding.source.to_string());
                }
                Some(name) => {
                    if let Some(kind) = helpers.named(name) {
                        self.helpers.insert(binding.key, kind);
                    }
                }
            }
        }
        self.bundled = bundled;
        self
    }

    /// Whether some provider the module requires exports a helper of
    /// `kind` ([`Self::with_bundled_helpers`]).
    pub(crate) fn has_bundled_helper(&self, kind: TranspilerHelperKind) -> bool {
        self.bundled
            .values()
            .any(|helpers| helpers.exports_kind(kind))
    }

    /// The source of `require("src")`, with an unresolved `require`.
    fn require_source<'a>(&self, expr: &'a Expr) -> Option<&'a str> {
        require_source(expr, self.unresolved_mark)
    }

    /// The helper kind of a callee reached through a bundled provider.
    fn bundled_callee_kind(&self, callee: &Expr) -> Option<TranspilerHelperKind> {
        if self.bundled.is_empty() {
            return None;
        }
        match strip_parens(callee) {
            Expr::Member(member) => {
                let name = static_member_prop_name(&member.prop)?;
                let source = match strip_parens(&member.obj) {
                    Expr::Ident(namespace) => self
                        .bundled_namespaces
                        .get(&binding_key(namespace))?
                        .as_str(),
                    object => self.require_source(object)?,
                };
                self.bundled.get(source)?.named(name)
            }
            callee @ Expr::Call(_) => self.bundled.get(self.require_source(callee)?)?.value(),
            _ => None,
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

    pub(crate) fn swc_member_helpers_of_kind(
        &self,
        kind: TranspilerHelperKind,
    ) -> HashMap<BindingKey, TranspilerHelperKind> {
        self.swc_member_helpers
            .iter()
            .filter(|(_, helper_kind)| **helper_kind == kind)
            .map(|(key, helper_kind)| (key.clone(), *helper_kind))
            .collect()
    }

    pub(crate) fn swc_member_helper_namespaces_of_kind(
        &self,
        kind: TranspilerHelperKind,
    ) -> HashMap<BindingKey, TranspilerHelperKind> {
        self.swc_member_helper_namespaces
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

        let removable = removable_without_remaining_refs(module, &helper_keys);
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

        let removable = removable_without_remaining_var_declarator_refs(module, &helper_keys);
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

        if let Expr::Member(member) = callee {
            if member_prop_name(&member.prop, "_") {
                if let Expr::Ident(namespace) = member.obj.as_ref() {
                    if let Some(kind) = self
                        .swc_member_helpers
                        .get(&(namespace.sym.clone(), namespace.ctxt))
                    {
                        return Some(*kind);
                    }
                }
            }
        }

        tslib_member_helper_kind(callee, &self.tslib_namespaces)
            .or_else(|| {
                tslib_require_member_name(callee, self.unresolved_mark)
                    .and_then(tslib_helper_name_kind)
            })
            .or_else(|| self.bundled_callee_kind(callee))
    }

    pub(crate) fn is_helper_callee(&self, callee: &Expr, kind: TranspilerHelperKind) -> bool {
        self.helper_callee_kind(callee) == Some(kind)
    }

    pub(crate) fn unresolved_mark(&self) -> Option<Mark> {
        self.unresolved_mark
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
            return HashMap::default();
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

/// The source of `require("src")`, with an unresolved `require`.
fn require_source(expr: &Expr, unresolved_mark: Option<Mark>) -> Option<&str> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let swc_core::ecma::ast::Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Ident(require) = strip_parens(callee) else {
        return None;
    };
    if !is_unresolved_or_unguarded_ident(require, "require", unresolved_mark) {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    if arg.spread.is_some() {
        return None;
    }
    match strip_parens(&arg.expr) {
        Expr::Lit(Lit::Str(source)) => source.value.as_str(),
        _ => None,
    }
}

/// A top-level binding of a required value or of one static member of it
/// ([`top_level_required_bindings`]).
pub(crate) struct RequiredBinding<'a> {
    pub(crate) key: BindingKey,
    /// The source the binding requires.
    pub(crate) source: &'a str,
    /// `None` for `var x = require("src")`; the member's name for
    /// `var x = require("src").name`.
    pub(crate) member: Option<&'a str>,
}

/// The top-level `var x = require("src")` and `var x = require("src").name`
/// declarators whose binding no assignment, update, or loop head writes.
/// A second `var` declarator of the same name is not counted as a write
/// (see [`LocalHelperContext::with_bundled_helpers`]).
///
/// `UnEsm` trusts a bundled provider's helpers through these bindings, and
/// the unpack plan's use collector reads a value passed to one as passed to
/// that provider's export, so the two read the same bindings.
pub(crate) fn top_level_required_bindings(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> Vec<RequiredBinding<'_>> {
    let mut found = Vec::new();
    for item in &module.body {
        let swc_core::ecma::ast::ModuleItem::Stmt(swc_core::ecma::ast::Stmt::Decl(
            swc_core::ecma::ast::Decl::Var(var),
        )) = item
        else {
            continue;
        };
        for declarator in &var.decls {
            let (swc_core::ecma::ast::Pat::Ident(binding), Some(init)) =
                (&declarator.name, declarator.init.as_deref())
            else {
                continue;
            };
            let (source, member) = match strip_parens(init) {
                Expr::Call(_) => (require_source(init, unresolved_mark), None),
                Expr::Member(member) => match static_member_prop_name(&member.prop) {
                    Some(name) => (require_source(&member.obj, unresolved_mark), Some(name)),
                    None => continue,
                },
                _ => continue,
            };
            let Some(source) = source else {
                continue;
            };
            found.push(RequiredBinding {
                key: binding_key(&binding.id),
                source,
                member,
            });
        }
    }
    if !found.is_empty() {
        let written =
            crate::analysis::binding_uses::BindingUseIndex::collect_direct_write_bindings(module);
        found.retain(|binding| !written.contains(&binding.key));
    }
    found
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
