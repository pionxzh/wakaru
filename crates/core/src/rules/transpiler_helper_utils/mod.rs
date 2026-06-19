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
    module_has_babel_sub_helper_signals,
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
        let context_dependencies = self.helpers_of_kind(TranspilerHelperKind::HelperDependency);
        removable_roots
            .into_iter()
            .chain(helper_dependencies)
            .chain(context_dependencies)
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
mod tests {
    use super::matchers::{
        detect_helper_from_arrow, is_class_call_check_fn, is_object_without_properties_fn,
    };
    use super::*;
    use swc_core::common::{sync::Lrc, FileName, Globals, SourceMap, SyntaxContext, GLOBALS};
    use swc_core::ecma::ast::{
        CallExpr, Callee, Decl, Function, ImportSpecifier, ModuleDecl, ModuleItem, Pat, Stmt,
    };
    use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

    fn parse_module(code: &str) -> Module {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(Lrc::new(FileName::Anon), code.to_string());
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax::default()),
            Default::default(),
            StringInput::from(&*fm),
            None,
        );
        let mut parser = Parser::new_from(lexer);
        parser.parse_module().expect("failed to parse")
    }

    fn parse_first_function(code: &str) -> Function {
        let module = parse_module(code);
        for item in &module.body {
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) = item {
                return *fn_decl.function.clone();
            }
        }
        panic!("no function declaration found in source");
    }

    fn module_has_function(module: &Module, name: &str) -> bool {
        module.body.iter().any(|item| {
            matches!(
                item,
                ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                    if fn_decl.ident.sym.as_ref() == name
            )
        })
    }

    fn module_has_var(module: &Module, name: &str) -> bool {
        module.body.iter().any(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                return false;
            };
            var.decls.iter().any(
                |decl| matches!(&decl.name, Pat::Ident(binding) if binding.id.sym.as_ref() == name),
            )
        })
    }

    fn module_has_import_local(module: &Module, name: &str) -> bool {
        module.body.iter().any(|item| {
            let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
                return false;
            };
            import.specifiers.iter().any(|specifier| match specifier {
                ImportSpecifier::Default(default) => default.local.sym.as_ref() == name,
                ImportSpecifier::Named(named) => named.local.sym.as_ref() == name,
                ImportSpecifier::Namespace(namespace) => namespace.local.sym.as_ref() == name,
            })
        })
    }

    #[test]
    fn local_helper_context_collects_ts_helpers() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                import { __spreadArray as importedSpread } from "tslib";
                import * as tslibNs from "tslib";
                import { __awaiter as importedAwaiter } from "tslib";
                var aliasedAwaiter = (this && this.__awaiter) || function(thisArg, _arguments, P, generator) {
                    return new (P || (P = Promise))(function(resolve) {
                        resolve(generator.apply(thisArg, _arguments || []).next());
                    });
                };
                var aliasedGenerator = (this && this.__generator) || function(thisArg, body) {
                    return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
                };
                function e(thisArg, body) {
                    var state = { label: 0, sent: function() {}, trys: [], ops: [] };
                    return body.call(thisArg, state);
                }
                function realStateMachine(user, options) {
                    var state = { label: 0, trys: [], ops: [] };
                    return options(state);
                }
                var inlineSpread = (this && this.__spreadArray) || function(to, from, pack) {
                    return to.concat(from);
                };
                var tslib_1 = require("tslib");
                var requiredSpread = require("tslib").__spreadArray;
                var requiredAwaiter = require("tslib").__awaiter;
                var namespaceSpread = tslib_1.__spreadArray;
                var namespaceAwaiter = tslib_1.__awaiter;
                var notSpread = customSpreadArray;
                var fakeAssign = (this && this.__assign) || customAssign;
                "#,
            );
            let helpers =
                LocalHelperContext::collect(&module).ts_helpers_of_kind(TsHelperKind::SpreadArray);

            assert_eq!(helpers.len(), 4);
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "importedSpread"));
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "inlineSpread"));
            assert!(helpers
                .iter()
                .any(|(sym, _)| sym.as_ref() == "requiredSpread"));
            assert!(!helpers.iter().any(|(sym, _)| sym.as_ref() == "notSpread"));

            let context = LocalHelperContext::collect(&module);
            let inline_helpers: HashMap<_, _> = context
                .ts_helpers
                .iter()
                .filter(|(_, helper)| helper.source == TsHelperSource::Inline)
                .map(|(key, helper)| (key.clone(), helper.kind))
                .collect();
            assert_eq!(
                inline_helpers
                    .get(&(Atom::from("aliasedAwaiter"), SyntaxContext::empty())),
                Some(&TsHelperKind::Awaiter)
            );
            assert_eq!(
                inline_helpers
                    .get(&(Atom::from("aliasedGenerator"), SyntaxContext::empty())),
                Some(&TsHelperKind::Generator)
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("e"), SyntaxContext::empty())),
                Some(&TsHelperKind::Generator)
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("realStateMachine"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("importedAwaiter"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("requiredAwaiter"), SyntaxContext::empty())),
                None
            );
            assert_eq!(
                inline_helpers.get(&(Atom::from("namespaceAwaiter"), SyntaxContext::empty())),
                None
            );

            let awaiter_helpers = context.ts_helpers_of_kind(TsHelperKind::Awaiter);
            assert_eq!(awaiter_helpers.len(), 4);

            let assign_helpers = context.ts_helpers_of_kind(TsHelperKind::Assign);
            assert!(
                !assign_helpers
                    .iter()
                    .any(|(sym, _)| sym.as_ref() == "fakeAssign"),
                "name-only inline helper candidates should not be collected"
            );

            assert!(
                context
                    .tslib_namespaces()
                    .contains(&(Atom::from("tslibNs"), SyntaxContext::empty()))
            );
            assert!(
                context
                    .tslib_namespaces()
                    .contains(&(Atom::from("tslib_1"), SyntaxContext::empty()))
            );
        });
    }

    #[test]
    fn generated_function_with_label_property_is_not_ts_generator_helper() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                function L(effect, parentEffectId, label = "", extra) {
                    monitor.effectTriggered({
                        effectId: id,
                        parentEffectId,
                        label,
                        effect
                    });
                    use(effect, extra);
                }
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            assert!(
                !context
                    .ts_helpers_of_kind(TsHelperKind::Generator)
                    .iter()
                    .any(|(sym, _)| sym.as_ref() == "L"),
                "ordinary generated-looking functions with a label property are not TS generator helpers"
            );
        });
    }

    #[test]
    fn local_helper_context_collects_helper_dependencies() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                function root(value) {
                    return dep(value);
                }
                function dep(value) {
                    return leaf(value);
                }
                function leaf(value) {
                    return value;
                }
                function unrelated(value) {
                    return dep(value);
                }
                "#,
            );
            let context = LocalHelperContext::collect(&module);
            let roots = HashMap::from([(
                (Atom::from("root"), SyntaxContext::empty()),
                TranspilerHelperKind::SlicedToArray,
            )]);

            let dependencies = context.helper_dependencies(&module, &roots);

            assert_eq!(
                dependencies.get(&(Atom::from("dep"), SyntaxContext::empty())),
                Some(&TranspilerHelperKind::HelperDependency)
            );
            assert_eq!(
                dependencies.get(&(Atom::from("leaf"), SyntaxContext::empty())),
                Some(&TranspilerHelperKind::HelperDependency)
            );
            assert!(!dependencies.contains_key(&(Atom::from("root"), SyntaxContext::empty())));
            assert!(!dependencies.contains_key(&(Atom::from("unrelated"), SyntaxContext::empty())));
        });
    }

    #[test]
    fn removes_helpers_without_remaining_refs_only_when_unused() {
        GLOBALS.set(&Globals::new(), || {
            let mut unused = parse_module(
                r#"
                function helper(value) {
                    return value;
                }
                const value = 1;
                "#,
            );
            let helpers = HashMap::from([(
                (Atom::from("helper"), SyntaxContext::empty()),
                TranspilerHelperKind::ClassCallCheck,
            )]);

            remove_helpers_without_remaining_refs(&mut unused, helpers);

            assert!(!module_has_function(&unused, "helper"));

            let mut referenced = parse_module(
                r#"
                function helper(value) {
                    return value;
                }
                helper(1);
                "#,
            );
            let helpers = HashMap::from([(
                (Atom::from("helper"), SyntaxContext::empty()),
                TranspilerHelperKind::ClassCallCheck,
            )]);

            remove_helpers_without_remaining_refs(&mut referenced, helpers);

            assert!(module_has_function(&referenced, "helper"));
        });
    }

    #[test]
    fn removes_helper_dependencies_with_consumed_root() {
        GLOBALS.set(&Globals::new(), || {
            let mut module = parse_module(
                r#"
                function root(value) {
                    return dep(value);
                }
                function dep(value) {
                    return value;
                }
                function unrelated(value) {
                    return value;
                }
                "#,
            );
            let context = LocalHelperContext::collect(&module);
            let roots = HashMap::from([(
                (Atom::from("root"), SyntaxContext::empty()),
                TranspilerHelperKind::SlicedToArray,
            )]);

            context.remove_helpers_with_dependencies(&mut module, roots);

            assert!(!module_has_function(&module, "root"));
            assert!(!module_has_function(&module, "dep"));
            assert!(module_has_function(&module, "unrelated"));
        });
    }

    #[test]
    fn removes_unused_inline_ts_helpers_by_kind() {
        GLOBALS.set(&Globals::new(), || {
            let mut module = parse_module(
                r#"
                var __awaiter = (this && this.__awaiter) || function (thisArg, _arguments, P, generator) {
                    return new (P || (P = Promise))(function(resolve) {
                        resolve(generator.apply(thisArg, _arguments || []).next());
                    });
                };
                var __generator = (this && this.__generator) || function (thisArg, body) {
                    return body.call(thisArg, { label: 0, sent: function() {}, trys: [], ops: [] });
                };
                import { __awaiter as importedAwaiter } from "tslib";
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            context.remove_unused_inline_ts_helpers(
                &mut module,
                &[TsHelperKind::Awaiter, TsHelperKind::Generator],
            );

            assert!(!module_has_var(&module, "__awaiter"));
            assert!(!module_has_var(&module, "__generator"));
            assert!(module_has_import_local(&module, "importedAwaiter"));
        });
    }

    #[test]
    fn removes_unused_ts_helper_bindings_by_kind() {
        GLOBALS.set(&Globals::new(), || {
            let mut module = parse_module(
                r#"
                import { __spreadArray } from "tslib";
                var spread = require("tslib").__spreadArray;
                var kept = require("tslib").__spreadArray;
                kept([], [], true);
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            context.remove_unused_ts_helper_bindings(&mut module, TsHelperKind::SpreadArray);

            assert!(!module_has_import_local(&module, "__spreadArray"));
            assert!(!module_has_var(&module, "spread"));
            assert!(module_has_var(&module, "kept"));
        });
    }

    #[test]
    fn local_helper_context_records_direct_tslib_require_member_calls() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var a = require("tslib").__importDefault(require("a"));
                var b = require("tslib").__importStar(require("b"));
                var c = require("tslib").__read(values, 2);
                var d = require("not-tslib").__read(values, 2);
                "#,
            );
            let context = LocalHelperContext::collect(&module);

            assert!(
                context.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireDefault)
            );
            assert!(
                context.has_tslib_require_member_call(TranspilerHelperKind::InteropRequireWildcard)
            );
            assert!(context.has_tslib_require_member_call(TranspilerHelperKind::SlicedToArray));
            assert!(!context.has_tslib_require_member_call(TranspilerHelperKind::ObjectSpread));
        });
    }

    #[test]
    fn local_helper_context_matches_helper_callees() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                import * as tslibNs from "tslib";
                var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");
                var local = _interopRequireDefault(require("local"));
                var namespaced = tslibNs.__importDefault(require("namespaced"));
                var direct = require("tslib").__importDefault(require("direct"));
                var unrelated = maybe.__importDefault(require("unrelated"));
                "#,
            );
            let context = LocalHelperContext::collect(&module);
            let callees: Vec<_> = module
                .body
                .iter()
                .filter_map(|item| {
                    let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
                        return None;
                    };
                    let decl = var.decls.first()?;
                    let Expr::Call(call) = decl.init.as_deref()? else {
                        return None;
                    };
                    let Callee::Expr(callee) = &call.callee else {
                        return None;
                    };
                    Some(callee.as_ref())
                })
                .collect();

            assert_eq!(
                context.helper_callee_kind(callees[1]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(
                context.helper_callee_kind(callees[2]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(
                context.helper_callee_kind(callees[3]),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
            assert_eq!(context.helper_callee_kind(callees[4]), None);
        });
    }

    #[test]
    fn local_helper_context_collects_typeof_polyfill_helper() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var _typeof = typeof Symbol == "function" && typeof Symbol.iterator == "symbol"
                    ? function(e) { return typeof e; }
                    : function(e) { return e && typeof Symbol == "function" ? "symbol" : typeof e; };
                var notTypeof = typeof window != "undefined" ? function(e) { return typeof e; } : function(e) { return e; };
                "#,
            );
            let helpers = LocalHelperContext::collect(&module).helpers_of_kind(TranspilerHelperKind::Typeof);

            assert_eq!(helpers.len(), 1);
            assert!(helpers.contains_key(&(Atom::from("_typeof"), SyntaxContext::empty())));
            assert!(!helpers.contains_key(&(Atom::from("notTypeof"), SyntaxContext::empty())));
        });
    }

    #[test]
    fn local_helper_context_collects_tsc_private_field_helpers() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                function __classPrivateFieldGet(receiver, state, kind, f) {
                    return state.get(receiver);
                }
                var __classPrivateFieldSet = function(receiver, state, value, kind, f) {
                    return state.set(receiver, value), value;
                };
                var A4 = function(receiver, state, value, kind) {
                    return state.set(receiver, value), value;
                };
                "#,
            );
            let context = LocalHelperContext::collect(&module);
            let getters = context.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldGet);
            let setters = context.ts_helpers_of_kind(TsHelperKind::ClassPrivateFieldSet);

            assert!(
                getters.contains(&(Atom::from("__classPrivateFieldGet"), SyntaxContext::empty()))
            );
            assert!(
                setters.contains(&(Atom::from("__classPrivateFieldSet"), SyntaxContext::empty()))
            );
            assert!(!setters.contains(&(Atom::from("A4"), SyntaxContext::empty())));
        });
    }

    #[test]
    fn inline_legacy_spread_arrays_expression_matches_kind() {
        GLOBALS.set(&Globals::new(), || {
            let module = parse_module(
                r#"
                var out = (this && this.__spreadArrays || function () {
                    for (var s = 0, i = 0, il = arguments.length; i < il; i++) s += arguments[i].length;
                    for (var r = Array(s), k = 0, i = 0; i < il; i++)
                        for (var a = arguments[i], j = 0, jl = a.length; j < jl; j++, k++)
                            r[k] = a[j];
                    return r;
                })([head], items, [tail]);
                "#,
            );
            let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = &module.body[0] else {
                panic!("expected var decl");
            };
            let Expr::Call(call) = var.decls[0].init.as_deref().expect("init") else {
                panic!("expected call");
            };
            let Callee::Expr(callee) = &call.callee else {
                panic!("expected expr callee");
            };
            assert!(ts_expr_matches_helper_kind(
                callee,
                TsHelperKind::SpreadArrays
            ));
        });
    }

    #[test]
    fn class_call_check_canonical() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_no_block_wrapping() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b))
                        throw new TypeError("Cannot call a class as a function");
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_with_parens() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        throw new TypeError("Cannot call a class as a function");
                    }
                }"#,
            );
            assert!(is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_wrong_param_count() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a) {
                    if (!(a instanceof Foo)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_swapped_operands() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(b instanceof a)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_non_instanceof() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a === b)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_no_throw() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    if (!(a instanceof b)) {
                        console.log("bad");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn class_call_check_rejects_multiple_stmts() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _c(a, b) {
                    var x = 1;
                    if (!(a instanceof b)) {
                        throw new TypeError("nope");
                    }
                }"#,
            );
            assert!(!is_class_call_check_fn(&f));
        });
    }

    #[test]
    fn object_without_properties_spec_wrapper() {
        GLOBALS.set(&Globals::new(), || {
            let f = parse_first_function(
                r#"function _objectWithoutProperties(e, t) {
                    if (null == e) return {};
                    var o, r, i = _objectWithoutPropertiesLoose(e, t);
                    if (Object.getOwnPropertySymbols) {
                        var n = Object.getOwnPropertySymbols(e);
                        for (r = 0; r < n.length; r++)
                            o = n[r], -1 === t.indexOf(o) && {}.propertyIsEnumerable.call(e, o) && (i[o] = e[o]);
                    }
                    return i;
                }"#,
            );
            assert!(is_object_without_properties_fn(&f));
        });
    }

    // -----------------------------------------------------------------------
    // Inline (expression-site) helper detection
    //
    // These exercise `classify_inline_helper_call` directly so the shared
    // body-shape recognition is unit-tested independent of the rules that
    // consume it. Each test wraps a helper body in an IIFE: `(<callee>)(arg)`.
    // -----------------------------------------------------------------------

    /// Parse `var x = <call>;` and return the init call expression.
    fn parse_first_call(code: &str) -> CallExpr {
        let module = parse_module(code);
        for item in &module.body {
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
                if let Some(Expr::Call(call)) = var.decls.first().and_then(|d| d.init.as_deref()) {
                    return call.clone();
                }
            }
        }
        panic!("no call expression found in source");
    }

    fn classify_first_call(code: &str) -> Option<TranspilerHelperKind> {
        let call = parse_first_call(code);
        classify_inline_helper_call(&call).map(|(kind, _)| kind)
    }

    /// Classify the callee of the first call expression directly, regardless of
    /// argument count. Mirrors how multi-argument call sites (classCallCheck,
    /// objectWithoutProperties) invoke the shared API.
    fn classify_first_callee(code: &str) -> Option<TranspilerHelperKind> {
        let call = parse_first_call(code);
        let Callee::Expr(callee) = &call.callee else {
            panic!("expected expression callee");
        };
        classify_inline_callable(strip_parens(callee))
    }

    #[test]
    fn inline_interop_default_ternary_arrow() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_call(
                    r#"var x = ((e) => e && e.__esModule ? e : { default: e })(req);"#
                ),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn inline_interop_default_ternary_return_block() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_call(
                    r#"var x = (function(e) {
                        return e && e.__esModule ? e : { default: e };
                    })(req);"#
                ),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn inline_interop_default_if_return_arrow() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_call(
                    r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        return { default: e };
                    })(req);"#
                ),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn inline_interop_wildcard() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_call(
                    r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        var t = {};
                        if (e != null) {
                            for (var n in e) {
                                if (Object.prototype.hasOwnProperty.call(e, n)) { t[n] = e[n]; }
                            }
                        }
                        t.default = e;
                        return t;
                    })(req);"#
                ),
                Some(TranspilerHelperKind::InteropRequireWildcard)
            );
        });
    }

    #[test]
    fn inline_class_call_check_arrow() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_callee(
                    r#"var x = ((e, t) => {
                        if (!(e instanceof t)) { throw new TypeError("Cannot call a class as a function"); }
                    })(this, Foo);"#
                ),
                Some(TranspilerHelperKind::ClassCallCheck)
            );
        });
    }

    #[test]
    fn inline_class_call_check_fn_expr() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_callee(
                    r#"var x = (function(e, t) {
                        if (!(e instanceof t)) { throw new TypeError("nope"); }
                    })(this, Foo);"#
                ),
                Some(TranspilerHelperKind::ClassCallCheck)
            );
        });
    }

    #[test]
    fn inline_object_without_properties() {
        GLOBALS.set(&Globals::new(), || {
            assert_eq!(
                classify_first_callee(
                    r#"var x = ((e, t) => {
                        var n = {};
                        for (var r in e) {
                            t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
                        }
                        return n;
                    })(obj, ["a", "b"]);"#
                ),
                Some(TranspilerHelperKind::ObjectWithoutProperties)
            );
        });
    }

    #[test]
    fn inline_helper_rejects_non_helper_iife() {
        GLOBALS.set(&Globals::new(), || {
            // __esModule guard with side effects + fallback is NOT an interop helper.
            assert_eq!(
                classify_first_call(
                    r#"var x = ((e) => {
                        if (e && e.__esModule) { return e; }
                        sideEffect(e);
                        return fallback;
                    })(input);"#
                ),
                None
            );
            // Ordinary arithmetic IIFE.
            assert_eq!(
                classify_first_call(r#"var x = ((e) => { var a = e + 1; return a * 2; })(42);"#),
                None
            );
        });
    }

    #[test]
    fn inline_helper_rejects_multiple_args() {
        GLOBALS.set(&Globals::new(), || {
            // classify_inline_helper_call requires exactly one argument; the
            // two-arg classCallCheck/OWP framing is validated by the call sites.
            let call = parse_first_call(
                r#"var x = ((e, t) => {
                    if (!(e instanceof t)) { throw new TypeError("nope"); }
                })(this, Foo);"#,
            );
            assert!(classify_inline_helper_call(&call).is_none());
            // ...but classifying the callable directly still recognizes the shape.
            if let Callee::Expr(callee) = &call.callee {
                assert_eq!(
                    classify_inline_callable(strip_parens(callee)),
                    Some(TranspilerHelperKind::ClassCallCheck)
                );
            } else {
                panic!("expected expression callee");
            }
        });
    }

    // -- declaration-site arrow detection (detect_helper_from_arrow) -----------

    fn parse_first_arrow(code: &str) -> swc_core::ecma::ast::ArrowExpr {
        let module = parse_module(code);
        for item in &module.body {
            if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
                if let Some(Expr::Arrow(arrow)) = var.decls.first().and_then(|d| d.init.as_deref())
                {
                    return arrow.clone();
                }
            }
        }
        panic!("no arrow expression found in source");
    }

    #[test]
    fn arrow_decl_interop_default_ternary_expr() {
        GLOBALS.set(&Globals::new(), || {
            let arrow =
                parse_first_arrow(r#"var f = (e) => e && e.__esModule ? e : { default: e };"#);
            assert_eq!(
                detect_helper_from_arrow(&arrow, false),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn arrow_decl_interop_default_ternary_return_block() {
        GLOBALS.set(&Globals::new(), || {
            let arrow = parse_first_arrow(
                r#"var f = (e) => { return e && e.__esModule ? e : { default: e }; };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, false),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn arrow_decl_interop_default_if_return_block() {
        GLOBALS.set(&Globals::new(), || {
            let arrow = parse_first_arrow(
                r#"var f = (e) => { if (e && e.__esModule) return e; return { default: e }; };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, false),
                Some(TranspilerHelperKind::InteropRequireDefault)
            );
        });
    }

    #[test]
    fn arrow_decl_object_without_properties() {
        GLOBALS.set(&Globals::new(), || {
            let arrow = parse_first_arrow(
                r#"var f = (e, t) => {
                    var n = {};
                    for (var r in e) {
                        t.indexOf(r) >= 0 || Object.prototype.hasOwnProperty.call(e, r) && (n[r] = e[r]);
                    }
                    return n;
                };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, false),
                Some(TranspilerHelperKind::ObjectWithoutProperties)
            );
        });
    }

    #[test]
    fn arrow_decl_to_consumable_array_threads_has_sub_helpers() {
        GLOBALS.set(&Globals::new(), || {
            // Babel 7+ OR-chain dispatcher form is only a helper when the module
            // carries sub-helper signals — pins that has_sub_helpers is threaded
            // through the arrow path unchanged.
            let arrow = parse_first_arrow(
                r#"var f = (arr) => { return _arrayWithoutHoles(arr) || _iterableToArray(arr) || _nonIterableSpread(); };"#,
            );
            assert_eq!(
                detect_helper_from_arrow(&arrow, true),
                Some(TranspilerHelperKind::ToConsumableArray)
            );
            assert_eq!(detect_helper_from_arrow(&arrow, false), None);
        });
    }

    #[test]
    fn arrow_decl_non_helper_is_none() {
        GLOBALS.set(&Globals::new(), || {
            let arrow = parse_first_arrow(r#"var f = (e) => { var a = e + 1; return a * 2; };"#);
            assert_eq!(detect_helper_from_arrow(&arrow, false), None);
        });
    }
}
