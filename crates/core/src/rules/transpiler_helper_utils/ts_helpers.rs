//! TypeScript / tslib helper detection — the raw `TsHelperKind` channel.
//!
//! Kept separate from the Babel/SWC body-shape matchers: tslib helpers are
//! tracked as raw kinds and are often consumed directly by rules (e.g.
//! UnAsyncAwait matches detected `__awaiter` / `__generator` aliases rather than
//! mapping them to a semantic kind).

use std::collections::{HashMap, HashSet};

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignExpr, BinExpr, BinaryOp, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, Function, Ident,
    ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, Pat, PropName,
    Stmt, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::rules::helper_matcher::{binding_key, var_declarator_binding_key};
use crate::utils::paren::strip_parens;

use super::*;

pub(super) fn collect_ts_helpers(
    module: &Module,
    tslib_namespaces: &HashSet<BindingKey>,
    unresolved_mark: Option<Mark>,
) -> HashMap<BindingKey, TsHelperInfo> {
    let mut helpers = HashMap::new();

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl))) => {
                if let Some(kind) =
                    ts_private_helper_name_kind(fn_decl.ident.sym.as_ref(), &fn_decl.function)
                        .or_else(|| ts_generated_fn_helper_kind(&fn_decl.ident, &fn_decl.function))
                {
                    helpers.insert(
                        binding_key(&fn_decl.ident),
                        TsHelperInfo {
                            kind,
                            source: TsHelperSource::Inline,
                        },
                    );
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    if let Some((key, helper)) =
                        collect_ts_helper_from_var_decl(decl, tslib_namespaces, unresolved_mark)
                    {
                        helpers.insert(key, helper);
                    }
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                if !import.type_only && is_tslib_path(import.src.value.as_str().unwrap_or("")) =>
            {
                for specifier in &import.specifiers {
                    let ImportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let imported = named
                        .imported
                        .as_ref()
                        .map(export_name_to_atom)
                        .unwrap_or_else(|| named.local.sym.clone());
                    if let Some(kind) = ts_helper_name_kind(imported.as_ref()) {
                        helpers.insert(
                            binding_key(&named.local),
                            TsHelperInfo {
                                kind,
                                source: TsHelperSource::TslibImport,
                            },
                        );
                    }
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Fn(fn_decl) => {
                    if let Some(kind) =
                        ts_private_helper_name_kind(fn_decl.ident.sym.as_ref(), &fn_decl.function)
                            .or_else(|| {
                                ts_generated_fn_helper_kind(&fn_decl.ident, &fn_decl.function)
                            })
                    {
                        helpers.insert(
                            binding_key(&fn_decl.ident),
                            TsHelperInfo {
                                kind,
                                source: TsHelperSource::Inline,
                            },
                        );
                    }
                }
                Decl::Var(var) => {
                    for decl in &var.decls {
                        if let Some((key, helper)) =
                            collect_ts_helper_from_var_decl(decl, tslib_namespaces, unresolved_mark)
                        {
                            helpers.insert(key, helper);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    helpers
}
pub(crate) fn collect_inline_ts_helpers_deep(module: &Module) -> HashMap<BindingKey, TsHelperKind> {
    struct Collector {
        helpers: HashMap<BindingKey, TsHelperKind>,
    }

    impl Visit for Collector {
        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            if let Some(kind) =
                ts_private_helper_name_kind(fn_decl.ident.sym.as_ref(), &fn_decl.function)
                    .or_else(|| ts_generated_fn_helper_kind(&fn_decl.ident, &fn_decl.function))
            {
                self.helpers.insert(binding_key(&fn_decl.ident), kind);
            }
            fn_decl.visit_children_with(self);
        }

        fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
            if let Some((key, helper)) =
                collect_ts_helper_from_var_decl(decl, &HashSet::new(), None)
            {
                if helper.source == TsHelperSource::Inline {
                    self.helpers.insert(key, helper.kind);
                }
            }
            decl.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left {
                    if let Some(kind) =
                        ts_generated_values_callable_kind(&target.id, assign.right.as_ref())
                    {
                        self.helpers.insert(binding_key(&target.id), kind);
                    }
                }
            }
            assign.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        helpers: HashMap::new(),
    };
    module.visit_with(&mut collector);
    collector.helpers
}
fn collect_ts_helper_from_var_decl(
    decl: &VarDeclarator,
    tslib_namespaces: &HashSet<BindingKey>,
    unresolved_mark: Option<Mark>,
) -> Option<(BindingKey, TsHelperInfo)> {
    let init = decl.init.as_deref()?;
    let key = var_declarator_binding_key(decl)?;
    if let Some(kind) = ts_private_helper_decl_kind(key.0.as_ref(), init) {
        return Some((
            key,
            TsHelperInfo {
                kind,
                source: TsHelperSource::Inline,
            },
        ));
    }

    if let Some(kind) = ts_inline_helper_kind(init) {
        return Some((
            key,
            TsHelperInfo {
                kind,
                source: TsHelperSource::Inline,
            },
        ));
    }

    if let Pat::Ident(binding) = &decl.name {
        if let Some(kind) = ts_generated_values_callable_kind(&binding.id, init) {
            return Some((
                key,
                TsHelperInfo {
                    kind,
                    source: TsHelperSource::Inline,
                },
            ));
        }
    }

    if let Some(kind) =
        tslib_require_member_name(init, unresolved_mark).and_then(ts_helper_name_kind)
    {
        return Some((
            key,
            TsHelperInfo {
                kind,
                source: TsHelperSource::TslibRequire,
            },
        ));
    }

    let kind = tslib_namespace_member_name(init, tslib_namespaces).and_then(ts_helper_name_kind)?;
    Some((
        key,
        TsHelperInfo {
            kind,
            source: TsHelperSource::TslibNamespace,
        },
    ))
}
pub(crate) fn tslib_helper_name_kind(name: &str) -> Option<TranspilerHelperKind> {
    match name {
        "__assign" => Some(TranspilerHelperKind::Extends),
        "__rest" => Some(TranspilerHelperKind::ObjectWithoutProperties),
        "__read" => Some(TranspilerHelperKind::SlicedToArray),
        "__importDefault" => Some(TranspilerHelperKind::InteropRequireDefault),
        "__importStar" => Some(TranspilerHelperKind::InteropRequireWildcard),
        _ => None,
    }
}
fn ts_helper_name_kind(name: &str) -> Option<TsHelperKind> {
    match name {
        "__awaiter" => Some(TsHelperKind::Awaiter),
        "__generator" => Some(TsHelperKind::Generator),
        "__values" | "_ts_values" => Some(TsHelperKind::Values),
        "__assign" => Some(TsHelperKind::Assign),
        "__rest" => Some(TsHelperKind::Rest),
        "__extends" => Some(TsHelperKind::Extends),
        "__importDefault" => Some(TsHelperKind::ImportDefault),
        "__importStar" => Some(TsHelperKind::ImportStar),
        "__createBinding" => Some(TsHelperKind::CreateBinding),
        "__setModuleDefault" => Some(TsHelperKind::SetModuleDefault),
        "__read" => Some(TsHelperKind::Read),
        "__spread" => Some(TsHelperKind::Spread),
        "__spreadArrays" => Some(TsHelperKind::SpreadArrays),
        "__spreadArray" => Some(TsHelperKind::SpreadArray),
        _ => None,
    }
}
pub(crate) fn is_tslib_path(path: &str) -> bool {
    matches!(path, "tslib" | "tslib/tslib.es6.js" | "tslib/tslib.js")
}
pub(crate) fn collect_tslib_namespace_bindings(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> HashSet<BindingKey> {
    let mut bindings = HashSet::new();

    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import))
                if !import.type_only && is_tslib_path(import.src.value.as_str().unwrap_or("")) =>
            {
                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Default(default) => {
                            bindings.insert(binding_key(&default.local));
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            bindings.insert(binding_key(&namespace.local));
                        }
                        ImportSpecifier::Named(_) => {}
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                for decl in &var.decls {
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if !is_tslib_require_call(init, unresolved_mark) {
                        continue;
                    }
                    if let Some(key) = var_declarator_binding_key(decl) {
                        bindings.insert(key);
                    }
                }
            }
            _ => {}
        }
    }

    bindings
}
pub(crate) fn tslib_namespace_member_name<'a>(
    expr: &'a Expr,
    namespaces: &HashSet<BindingKey>,
) -> Option<&'a str> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    let Expr::Ident(obj) = strip_parens(&member.obj) else {
        return None;
    };
    if !namespaces.contains(&binding_key(obj)) {
        return None;
    }
    static_member_prop_name(&member.prop)
}
pub(crate) fn is_tslib_spread_array_member(expr: &Expr, namespaces: &HashSet<BindingKey>) -> bool {
    tslib_namespace_member_name(expr, namespaces) == Some("__spreadArray")
}
pub(crate) fn tslib_member_helper_kind(
    expr: &Expr,
    namespaces: &HashSet<BindingKey>,
) -> Option<TranspilerHelperKind> {
    tslib_helper_name_kind(tslib_namespace_member_name(expr, namespaces)?)
}
pub(crate) fn tslib_member_ts_helper_kind(
    expr: &Expr,
    namespaces: &HashSet<BindingKey>,
) -> Option<TsHelperKind> {
    ts_helper_name_kind(tslib_namespace_member_name(expr, namespaces)?)
}
pub(crate) fn tslib_require_member_name(
    expr: &Expr,
    unresolved_mark: Option<Mark>,
) -> Option<&str> {
    let Expr::Member(member) = strip_parens(expr) else {
        return None;
    };
    if !is_tslib_require_call(&member.obj, unresolved_mark) {
        return None;
    }
    static_member_prop_name(&member.prop)
}
pub(crate) fn tslib_require_ts_helper_kind(
    expr: &Expr,
    unresolved_mark: Option<Mark>,
) -> Option<TsHelperKind> {
    tslib_require_member_name(expr, unresolved_mark).and_then(ts_helper_name_kind)
}
pub(crate) fn tslib_require_ts_helper_kind_with_mark(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<TsHelperKind> {
    tslib_require_ts_helper_kind(expr, Some(unresolved_mark))
}
pub(crate) fn is_tslib_require_expr_with_mark(expr: &Expr, unresolved_mark: Mark) -> bool {
    is_tslib_require_call(expr, Some(unresolved_mark))
}
pub(super) fn collect_tslib_require_member_calls(
    module: &Module,
    unresolved_mark: Option<Mark>,
) -> HashSet<TranspilerHelperKind> {
    struct Finder {
        kinds: HashSet<TranspilerHelperKind>,
        unresolved_mark: Option<Mark>,
    }

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Some(kind) = tslib_require_member_name(callee.as_ref(), self.unresolved_mark)
                    .and_then(tslib_helper_name_kind)
                {
                    self.kinds.insert(kind);
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        kinds: HashSet::new(),
        unresolved_mark,
    };
    module.visit_with(&mut finder);
    finder.kinds
}
pub(super) fn detect_helper_from_tslib_require_member(
    member: &MemberExpr,
    unresolved_mark: Option<Mark>,
) -> Option<TranspilerHelperKind> {
    if !is_tslib_require_call(&member.obj, unresolved_mark) {
        return None;
    }
    tslib_helper_name_kind(static_member_prop_name(&member.prop)?)
}
fn ts_inline_helper_kind(expr: &Expr) -> Option<TsHelperKind> {
    let (name, fallback) = ts_inline_helper_parts(expr)?;
    let kind = ts_helper_name_kind(name)?;
    ts_inline_helper_fallback_matches(fallback, kind).then_some(kind)
}
pub(crate) fn ts_expr_matches_helper_kind(expr: &Expr, kind: TsHelperKind) -> bool {
    ts_inline_helper_kind(expr) == Some(kind)
}
fn ts_inline_helper_parts(expr: &Expr) -> Option<(&str, &Expr)> {
    let expr = strip_parens(expr);
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };

    let left = strip_parens(left);
    let Expr::Bin(and_bin) = left else {
        return None;
    };
    if and_bin.op != BinaryOp::LogicalAnd {
        return None;
    }

    let and_left = strip_parens(and_bin.left.as_ref());
    let and_right = strip_parens(and_bin.right.as_ref());

    if !matches!(and_left, Expr::This(_)) {
        return None;
    }

    let Expr::Member(MemberExpr {
        obj,
        prop: MemberProp::Ident(prop),
        ..
    }) = and_right
    else {
        return None;
    };
    matches!(obj.as_ref(), Expr::This(_)).then_some((prop.sym.as_ref(), strip_parens(right)))
}
fn ts_inline_helper_fallback_matches(expr: &Expr, kind: TsHelperKind) -> bool {
    if let Expr::Cond(cond) = strip_parens(expr) {
        return ts_inline_helper_fallback_matches(&cond.cons, kind)
            || ts_inline_helper_fallback_matches(&cond.alt, kind);
    }

    let Some((param_len, body)) = ts_helper_callable_body(expr) else {
        return false;
    };
    let signals = collect_ts_helper_body_signals(body);
    match kind {
        TsHelperKind::Awaiter => {
            param_len >= 4 && (signals.promise || signals.generator_apply || signals.next_call)
        }
        TsHelperKind::Generator => {
            param_len >= 2 && (signals.label_prop || signals.trys_prop || signals.ops_prop)
        }
        // `__values` / `_ts_values`: single iterable param, grabs `Symbol.iterator`,
        // throws `TypeError` when the value is not iterable.
        TsHelperKind::Values => param_len == 1 && signals.symbol_iterator && signals.type_error,
        TsHelperKind::Assign => {
            signals.object_assign || (signals.arguments_ref && signals.has_own_property)
        }
        TsHelperKind::Rest => signals.has_own_property || signals.object_get_own_property_symbols,
        TsHelperKind::Extends => {
            signals.object_set_prototype_of || signals.proto_prop || signals.prototype_prop
        }
        TsHelperKind::ImportDefault => signals.es_module_prop && signals.default_prop,
        TsHelperKind::ImportStar => {
            signals.own_keys_loop
                || signals.create_binding_call
                || signals.set_module_default_call
                || (signals.default_prop && signals.has_own_property)
        }
        TsHelperKind::CreateBinding => {
            signals.object_define_property && (signals.get_prop || signals.enumerable_prop)
        }
        TsHelperKind::SetModuleDefault => signals.object_define_property && signals.default_prop,
        TsHelperKind::Read => signals.iterator_prop && signals.next_call,
        TsHelperKind::Spread => signals.arguments_ref && signals.concat_call,
        TsHelperKind::SpreadArrays => signals.arguments_ref && signals.array_constructor,
        TsHelperKind::SpreadArray => signals.concat_call,
        TsHelperKind::ClassPrivateFieldGet | TsHelperKind::ClassPrivateFieldSet => false,
    }
}
fn ts_helper_callable_body(expr: &Expr) -> Option<(usize, &[Stmt])> {
    match expr {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            Some((fn_expr.function.params.len(), body.stmts.as_slice()))
        }
        Expr::Arrow(arrow) => {
            let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
                return None;
            };
            Some((arrow.params.len(), body.stmts.as_slice()))
        }
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            ts_helper_callable_body(strip_parens(callee))
        }
        _ => None,
    }
}
#[derive(Default)]
struct TsHelperBodySignals {
    arguments_ref: bool,
    array_constructor: bool,
    concat_call: bool,
    create_binding_call: bool,
    default_prop: bool,
    enumerable_prop: bool,
    es_module_prop: bool,
    generator_apply: bool,
    get_prop: bool,
    has_own_property: bool,
    iterator_prop: bool,
    label_prop: bool,
    next_call: bool,
    object_assign: bool,
    object_define_property: bool,
    object_get_own_property_symbols: bool,
    object_set_prototype_of: bool,
    ops_prop: bool,
    own_keys_loop: bool,
    promise: bool,
    proto_prop: bool,
    prototype_prop: bool,
    set_module_default_call: bool,
    symbol_iterator: bool,
    trys_prop: bool,
    type_error: bool,
}
fn collect_ts_helper_body_signals(stmts: &[Stmt]) -> TsHelperBodySignals {
    struct SignalVisitor {
        signals: TsHelperBodySignals,
    }

    impl Visit for SignalVisitor {
        fn visit_ident(&mut self, ident: &Ident) {
            match ident.sym.as_ref() {
                "arguments" => self.signals.arguments_ref = true,
                "__createBinding" => self.signals.create_binding_call = true,
                "__setModuleDefault" => self.signals.set_module_default_call = true,
                "TypeError" => self.signals.type_error = true,
                _ => {}
            }
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if is_symbol_member(member, "iterator") {
                self.signals.symbol_iterator = true;
            }
            if is_object_member(member, "assign") {
                self.signals.object_assign = true;
            }
            if is_object_member(member, "defineProperty") {
                self.signals.object_define_property = true;
            }
            if is_object_member(member, "getOwnPropertySymbols") {
                self.signals.object_get_own_property_symbols = true;
            }
            if is_object_member(member, "setPrototypeOf") {
                self.signals.object_set_prototype_of = true;
            }
            match static_member_prop_name(&member.prop) {
                Some("__esModule") => self.signals.es_module_prop = true,
                Some("__proto__") => self.signals.proto_prop = true,
                Some("concat") => self.signals.concat_call = true,
                Some("default") => self.signals.default_prop = true,
                Some("enumerable") => self.signals.enumerable_prop = true,
                Some("get") => self.signals.get_prop = true,
                Some("hasOwnProperty") => self.signals.has_own_property = true,
                Some("iterator") => self.signals.iterator_prop = true,
                Some("label") => self.signals.label_prop = true,
                Some("next") => self.signals.next_call = true,
                Some("ops") => self.signals.ops_prop = true,
                Some("prototype") => self.signals.prototype_prop = true,
                Some("trys") => self.signals.trys_prop = true,
                _ => {}
            }
            member.visit_children_with(self);
        }

        fn visit_lit(&mut self, lit: &Lit) {
            if let Lit::Str(s) = lit {
                match s.value.as_str() {
                    Some("__esModule") => self.signals.es_module_prop = true,
                    Some("default") => self.signals.default_prop = true,
                    Some("enumerable") => self.signals.enumerable_prop = true,
                    Some("get") => self.signals.get_prop = true,
                    _ => {}
                }
            }
        }

        fn visit_prop_name(&mut self, name: &PropName) {
            match prop_name_as_str(name) {
                Some("__esModule") => self.signals.es_module_prop = true,
                Some("__proto__") => self.signals.proto_prop = true,
                Some("default") => self.signals.default_prop = true,
                Some("enumerable") => self.signals.enumerable_prop = true,
                Some("get") => self.signals.get_prop = true,
                Some("iterator") => self.signals.iterator_prop = true,
                Some("label") => self.signals.label_prop = true,
                Some("ops") => self.signals.ops_prop = true,
                Some("trys") => self.signals.trys_prop = true,
                _ => {}
            }
            name.visit_children_with(self);
        }

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if matches!(strip_parens(callee), Expr::Ident(id) if id.sym.as_ref() == "Array") {
                    self.signals.array_constructor = true;
                }
                if matches!(strip_parens(callee), Expr::Ident(id) if id.sym.as_ref() == "Promise") {
                    self.signals.promise = true;
                }
                if is_member_call(callee, "apply") {
                    self.signals.generator_apply = true;
                }
            }
            call.visit_children_with(self);
        }

        fn visit_new_expr(&mut self, new_expr: &swc_core::ecma::ast::NewExpr) {
            if matches!(strip_parens(&new_expr.callee), Expr::Ident(id) if id.sym.as_ref() == "Promise")
            {
                self.signals.promise = true;
            }
            new_expr.visit_children_with(self);
        }

        fn visit_for_in_stmt(&mut self, for_in: &swc_core::ecma::ast::ForInStmt) {
            self.signals.own_keys_loop = true;
            for_in.visit_children_with(self);
        }
    }

    let mut visitor = SignalVisitor {
        signals: TsHelperBodySignals::default(),
    };
    stmts.visit_with(&mut visitor);
    visitor.signals
}
fn is_tslib_require_call(expr: &Expr, unresolved_mark: Option<Mark>) -> bool {
    let Expr::Call(call) = strip_parens(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return false;
    };
    if !is_unresolved_or_unguarded_ident(id, "require", unresolved_mark)
        || call.args.len() != 1
        || call.args[0].spread.is_some()
    {
        return false;
    }
    let Expr::Lit(Lit::Str(s)) = call.args[0].expr.as_ref() else {
        return false;
    };
    is_tslib_path(s.value.as_str().unwrap_or(""))
}
fn ts_private_helper_decl_kind(name: &str, init: &Expr) -> Option<TsHelperKind> {
    let kind = match name {
        "_ts_generator" => TsHelperKind::Generator,
        "__classPrivateFieldGet" => TsHelperKind::ClassPrivateFieldGet,
        "__classPrivateFieldSet" => TsHelperKind::ClassPrivateFieldSet,
        _ => return None,
    };
    match kind {
        TsHelperKind::Generator => ts_inline_helper_fallback_matches(init, kind).then_some(kind),
        _ => expr_contains_tsc_private_helper_fn(init, kind).then_some(kind),
    }
}
fn ts_private_helper_name_kind(name: &str, function: &Function) -> Option<TsHelperKind> {
    let kind = match name {
        "_ts_generator" => TsHelperKind::Generator,
        "_ts_values" | "__values" => TsHelperKind::Values,
        "__classPrivateFieldGet" => TsHelperKind::ClassPrivateFieldGet,
        "__classPrivateFieldSet" => TsHelperKind::ClassPrivateFieldSet,
        _ => return None,
    };
    match kind {
        TsHelperKind::Generator | TsHelperKind::Values => {
            ts_function_matches_kind(function, kind).then_some(kind)
        }
        _ => is_tsc_private_helper_fn(function, kind).then_some(kind),
    }
}
fn ts_generated_fn_helper_kind(ident: &Ident, function: &Function) -> Option<TsHelperKind> {
    if !is_likely_generated_alias(ident.sym.as_ref()) {
        return None;
    }
    if ts_generated_generator_function_matches(function) {
        Some(TsHelperKind::Generator)
    } else if ts_values_function_matches(function) {
        // Minifiers strip the `_ts_values` / `__values` name, but the body shape
        // (single iterable param, `Symbol.iterator`, `TypeError`) is preserved.
        Some(TsHelperKind::Values)
    } else {
        None
    }
}
fn ts_generated_values_callable_kind(ident: &Ident, expr: &Expr) -> Option<TsHelperKind> {
    if !is_likely_generated_alias(ident.sym.as_ref()) {
        return None;
    }
    let (param_len, body) = ts_helper_callable_body(expr)?;
    let signals = collect_ts_helper_body_signals(body);
    (param_len == 1 && signals.symbol_iterator && signals.type_error)
        .then_some(TsHelperKind::Values)
}
fn ts_function_matches_kind(function: &Function, kind: TsHelperKind) -> bool {
    match kind {
        TsHelperKind::Generator => ts_generator_state_function_matches(function),
        TsHelperKind::Values => ts_values_function_matches(function),
        _ => false,
    }
}
fn ts_values_function_matches(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let signals = collect_ts_helper_body_signals(&body.stmts);
    function.params.len() == 1 && signals.symbol_iterator && signals.type_error
}
fn ts_generator_state_function_matches(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    let signals = collect_ts_helper_body_signals(&body.stmts);
    function.params.len() >= 2 && signals.label_prop && signals.trys_prop && signals.ops_prop
}
fn ts_generated_generator_function_matches(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    if !ts_generator_state_function_matches(function) {
        return false;
    }
    let Some(body_param) = function.params.get(1).and_then(|param| match &param.pat {
        Pat::Ident(binding) => Some(binding_key(&binding.id)),
        _ => None,
    }) else {
        return false;
    };

    struct BodyCallFinder {
        body_param: BindingKey,
        found: bool,
    }

    impl Visit for BodyCallFinder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = strip_parens(callee) {
                    if matches!(static_member_prop_name(&member.prop), Some("call"))
                        && matches!(
                            member.obj.as_ref(),
                            Expr::Ident(obj) if binding_key(obj) == self.body_param
                        )
                    {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = BodyCallFinder {
        body_param,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}
fn expr_contains_tsc_private_helper_fn(expr: &Expr, kind: TsHelperKind) -> bool {
    struct Finder {
        kind: TsHelperKind,
        found: bool,
    }

    impl Visit for Finder {
        fn visit_function(&mut self, function: &Function) {
            if is_tsc_private_helper_fn(function, self.kind) {
                self.found = true;
            }
        }
    }

    let mut finder = Finder { kind, found: false };
    expr.visit_with(&mut finder);
    finder.found
}
fn is_tsc_private_helper_fn(function: &Function, kind: TsHelperKind) -> bool {
    let Some(state_key) = function.params.get(1).and_then(|param| match &param.pat {
        Pat::Ident(binding) => Some(binding_key(&binding.id)),
        _ => None,
    }) else {
        return false;
    };

    struct AccessFinder {
        state_key: BindingKey,
        kind: TsHelperKind,
        found: bool,
    }

    impl Visit for AccessFinder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = callee.as_ref() {
                    if let Expr::Ident(obj) = member.obj.as_ref() {
                        let prop_matches = match self.kind {
                            TsHelperKind::ClassPrivateFieldGet => {
                                matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "get")
                            }
                            TsHelperKind::ClassPrivateFieldSet => {
                                matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "set")
                            }
                            _ => false,
                        };
                        if prop_matches && binding_key(obj) == self.state_key {
                            self.found = true;
                            return;
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = AccessFinder {
        state_key,
        kind,
        found: false,
    };
    if let Some(body) = &function.body {
        body.visit_with(&mut finder);
    }
    finder.found
}
