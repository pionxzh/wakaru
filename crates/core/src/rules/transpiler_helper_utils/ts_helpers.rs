//! TypeScript / tslib helper detection — the raw `TsHelperKind` channel.
//!
//! Kept separate from the Babel/SWC body-shape matchers: tslib helpers are
//! tracked as raw kinds and are often consumed directly by rules (e.g.
//! UnAsyncAwait matches detected `__awaiter` / `__generator` aliases rather than
//! mapping them to a semantic kind).

use crate::collections::{HashMap, HashSet};

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    ArrowExpr, ArrowFunctionBody, AssignExpr, BinExpr, BinaryOp, CallExpr, Callee, Decl, Expr,
    Function, Ident, ImportSpecifier, Lit, MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem,
    Pat, PropName, Stmt, VarDeclarator,
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
    let mut helpers = HashMap::default();

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
                    } else if let Some(key) =
                        detect_ts_extends_sequence(module, decl, unresolved_mark)
                    {
                        helpers.insert(
                            key,
                            TsHelperInfo {
                                kind: TsHelperKind::Extends,
                                source: TsHelperSource::Inline,
                            },
                        );
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
                collect_ts_helper_from_var_decl(decl, &HashSet::default(), None)
            {
                if helper.source == TsHelperSource::Inline {
                    self.helpers.insert(key, helper.kind);
                }
            } else if let (Pat::Ident(binding), Some(init)) = (&decl.name, decl.init.as_deref()) {
                if let Some(kind) = ts_generated_fact_callable_kind(&binding.id, init) {
                    self.helpers.insert(binding_key(&binding.id), kind);
                }
            }
            decl.visit_children_with(self);
        }

        fn visit_assign_expr(&mut self, assign: &AssignExpr) {
            if assign.op == AssignOp::Assign {
                if let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left {
                    if let Some(kind) =
                        ts_generated_fact_callable_kind(&target.id, assign.right.as_ref())
                    {
                        self.helpers.insert(binding_key(&target.id), kind);
                    }
                }
            }
            assign.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        helpers: HashMap::default(),
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
        "__makeTemplateObject" => Some(TranspilerHelperKind::TaggedTemplateLiteral),
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
        "__asyncValues" => Some(TsHelperKind::AsyncValues),
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
        "__classPrivateFieldGet" => Some(TsHelperKind::ClassPrivateFieldGet),
        "__classPrivateFieldSet" => Some(TsHelperKind::ClassPrivateFieldSet),
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
    let mut bindings = HashSet::default();

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
        kinds: HashSet::default(),
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
// Terser lifts the ownKeys factory local and replaces the IIFE with a
// sequence. Its initialization can be discarded only while the lifted var
// remains private to this helper declaration.
pub(super) fn detect_ts_import_star_sequence(
    module: &Module,
    decl: &VarDeclarator,
    unresolved_mark: Option<Mark>,
) -> Option<BindingKey> {
    let key = var_declarator_binding_key(decl)?;
    let ("__importStar", fallback) = ts_inline_helper_parts(decl.init.as_deref()?)? else {
        return None;
    };
    let Expr::Seq(sequence) = strip_parens(fallback) else {
        return None;
    };
    let [initialize, callable] = sequence.exprs.as_slice() else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(initialize) else {
        return None;
    };
    if assign.op != swc_core::ecma::ast::AssignOp::Assign {
        return None;
    }
    let swc_core::ecma::ast::AssignTarget::Simple(swc_core::ecma::ast::SimpleAssignTarget::Ident(
        binding,
    )) = &assign.left
    else {
        return None;
    };
    let own_keys = binding_key(&binding.id);
    let Expr::Fn(factory) = strip_parens(&assign.right) else {
        return None;
    };
    let Expr::Fn(helper) = strip_parens(callable) else {
        return None;
    };
    if factory.function.is_async
        || factory.function.is_generator
        || helper.function.is_async
        || helper.function.is_generator
        || factory.function.params.len() != 1
        || helper.function.params.len() != 1
    {
        return None;
    }
    let factory_signals = collect_ts_helper_body_signals(&factory.function.body.as_ref()?.stmts);
    let helper_signals = collect_ts_helper_body_signals(&helper.function.body.as_ref()?.stmts);
    if !factory_signals.own_keys_loop
        || !factory_signals.has_own_property
        || !helper_signals.es_module_prop
    {
        return None;
    }
    ts_factory_local_is_private(module, &key, &own_keys, unresolved_mark).then_some(key)
}

// Like the import-star factory above, module-mode Terser can lift __extends'
// extendStatics local. Both functions and the module-wide ownership proof are
// required; the `this.__extends` marker alone does not identify a helper.
fn detect_ts_extends_sequence(
    module: &Module,
    decl: &VarDeclarator,
    unresolved_mark: Option<Mark>,
) -> Option<BindingKey> {
    let key = var_declarator_binding_key(decl)?;
    let ("__extends", fallback) = ts_inline_helper_parts(decl.init.as_deref()?)? else {
        return None;
    };
    let Expr::Seq(sequence) = strip_parens(fallback) else {
        return None;
    };
    let [initialize, callable] = sequence.exprs.as_slice() else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(initialize) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };
    let Expr::Fn(factory) = strip_parens(&assign.right) else {
        return None;
    };
    let Expr::Fn(helper) = strip_parens(callable) else {
        return None;
    };
    if factory.function.is_async
        || factory.function.is_generator
        || helper.function.is_async
        || helper.function.is_generator
        || factory.function.params.len() != 2
        || helper.function.params.len() != 2
    {
        return None;
    }
    let factory_signals = collect_ts_helper_body_signals(&factory.function.body.as_ref()?.stmts);
    let helper_signals = collect_ts_helper_body_signals(&helper.function.body.as_ref()?.stmts);
    if !factory_signals.object_set_prototype_of
        || !factory_signals.proto_prop
        || !factory_signals.has_own_property
        || !helper_signals.prototype_prop
        || !helper_signals.type_error
    {
        return None;
    }
    let local = binding_key(&binding.id);
    // The returned helper must actually call the lifted factory with its two
    // parameters; a marker plus unrelated prototype code is not sufficient.
    let [first, second] = helper.function.params.as_slice() else {
        return None;
    };
    let (Pat::Ident(first), Pat::Ident(second)) = (&first.pat, &second.pat) else {
        return None;
    };
    let calls_local = helper.function.body.as_ref()?.stmts.iter().any(|stmt| {
        let Stmt::Expr(statement) = stmt else { return false; };
        let Expr::Call(call) = strip_parens(&statement.expr) else { return false; };
        call.args.len() == 2 && call.args.iter().all(|arg| arg.spread.is_none())
            && matches!(&call.callee, Callee::Expr(callee) if matches!(strip_parens(callee), Expr::Ident(id) if binding_key(id) == local))
            && matches!(strip_parens(&call.args[0].expr), Expr::Ident(id) if id.to_id() == first.id.to_id())
            && matches!(strip_parens(&call.args[1].expr), Expr::Ident(id) if id.to_id() == second.id.to_id())
    });
    if !calls_local {
        return None;
    }
    ts_factory_local_is_private(module, &key, &local, unresolved_mark).then_some(key)
}

fn ts_factory_local_is_private(
    module: &Module,
    key: &BindingKey,
    local: &BindingKey,
    unresolved_mark: Option<Mark>,
) -> bool {
    // The reference scan below skips declarators by binding identity. Count
    // declarations throughout the module first, including hoisted `var` inside
    // blocks, so a second initializer cannot hide in that skipped set.
    let uses = crate::analysis::binding_uses::BindingUseIndex::collect_module_items(&module.body);
    if !uses.has_single_declaration(key) || !uses.has_single_declaration(local) {
        return false;
    }
    let mut declarations = 0;
    for item in &module.body {
        if let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item {
            for candidate in &var.decls {
                if var_declarator_binding_key(candidate).as_ref() == Some(local) {
                    if var.kind != swc_core::ecma::ast::VarDeclKind::Var || candidate.init.is_some()
                    {
                        return false;
                    }
                    declarations += 1;
                }
            }
        }
    }
    if declarations != 1 {
        return false;
    }
    struct DynamicLookup {
        unresolved_mark: Option<Mark>,
        found: bool,
    }
    impl Visit for DynamicLookup {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if matches!(&call.callee, Callee::Expr(callee) if matches!(strip_parens(callee), Expr::Ident(id) if is_unresolved_or_unguarded_ident(id, "eval", self.unresolved_mark)))
            {
                self.found = true;
            }
            call.visit_children_with(self);
        }
        fn visit_with_stmt(&mut self, _: &swc_core::ecma::ast::WithStmt) {
            self.found = true;
        }
    }
    let mut dynamic = DynamicLookup {
        unresolved_mark,
        found: false,
    };
    module.visit_with(&mut dynamic);
    if dynamic.found {
        return false;
    }
    let remaining = super::remaining_refs_outside_var_declarators(
        module,
        &HashSet::from_iter([local.clone()]),
        &HashSet::from_iter([key.clone(), local.clone()]),
    );
    remaining.is_empty()
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
        TsHelperKind::Values => ts_values_body_matches(param_len, body),
        TsHelperKind::AsyncValues => ts_async_values_body_matches(param_len, body),
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
        TsHelperKind::ClassPrivateFieldGet | TsHelperKind::ClassPrivateFieldSet => {
            expr_contains_tsc_private_helper_fn(expr, kind)
        }
    }
}
fn ts_helper_callable_body(expr: &Expr) -> Option<(usize, &[Stmt])> {
    match strip_parens(expr) {
        Expr::Fn(fn_expr) => {
            let body = fn_expr.function.body.as_ref()?;
            Some((fn_expr.function.params.len(), body.stmts.as_slice()))
        }
        Expr::Arrow(arrow) => {
            let ArrowFunctionBody::FunctionBody(body) = arrow.body.as_ref() else {
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
    symbol_async_iterator: bool,
    trys_prop: bool,
    type_error: bool,
}
fn collect_ts_helper_body_signals(stmts: &[Stmt]) -> TsHelperBodySignals {
    collect_ts_helper_signals(stmts, true)
}
/// Signals from the helper's own statements only; nested functions and arrows
/// are not entered.
fn collect_ts_helper_own_body_signals(stmts: &[Stmt]) -> TsHelperBodySignals {
    collect_ts_helper_signals(stmts, false)
}
fn collect_ts_helper_signals(stmts: &[Stmt], descend_into_functions: bool) -> TsHelperBodySignals {
    struct SignalVisitor {
        signals: TsHelperBodySignals,
        descend_into_functions: bool,
    }

    impl Visit for SignalVisitor {
        fn visit_function(&mut self, function: &Function) {
            if self.descend_into_functions {
                function.visit_children_with(self);
            }
        }

        fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
            if self.descend_into_functions {
                arrow.visit_children_with(self);
            }
        }

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
            if is_symbol_member(member, "asyncIterator") {
                self.signals.symbol_async_iterator = true;
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
        descend_into_functions,
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
        "__asyncValues" => TsHelperKind::AsyncValues,
        "__classPrivateFieldGet" => TsHelperKind::ClassPrivateFieldGet,
        "__classPrivateFieldSet" => TsHelperKind::ClassPrivateFieldSet,
        _ => return None,
    };
    match kind {
        TsHelperKind::Generator | TsHelperKind::Values | TsHelperKind::AsyncValues => {
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
    } else if ts_async_values_function_matches(function) {
        Some(TsHelperKind::AsyncValues)
    } else {
        None
    }
}
fn ts_generated_values_callable_kind(ident: &Ident, expr: &Expr) -> Option<TsHelperKind> {
    if !is_likely_generated_alias(ident.sym.as_ref()) {
        return None;
    }
    let (param_len, body) = ts_helper_callable_body(expr)?;
    if ts_values_body_matches(param_len, body) {
        Some(TsHelperKind::Values)
    } else if ts_async_values_body_matches(param_len, body) {
        Some(TsHelperKind::AsyncValues)
    } else {
        None
    }
}

/// Fact extraction may see minified tslib helpers as direct arrow/function
/// assignments rather than canonical `this && this.__helper || ...` fallbacks.
/// Require a generated binding plus the full body signature; registration under
/// the matching public helper name is checked separately by `facts.rs`.
fn ts_generated_fact_callable_kind(ident: &Ident, expr: &Expr) -> Option<TsHelperKind> {
    if !is_likely_generated_alias(ident.sym.as_ref())
        && !is_short_alphanumeric_minified_name(ident.sym.as_ref())
    {
        return None;
    }
    let (param_len, body) = ts_helper_callable_body(expr)?;
    let signals = collect_ts_helper_body_signals(body);
    if param_len >= 4 && signals.generator_apply && signals.next_call {
        Some(TsHelperKind::Awaiter)
    } else if param_len >= 2 && signals.label_prop && signals.trys_prop && signals.ops_prop {
        Some(TsHelperKind::Generator)
    } else if ts_values_body_matches(param_len, body) {
        Some(TsHelperKind::Values)
    } else if ts_async_values_body_matches(param_len, body) {
        Some(TsHelperKind::AsyncValues)
    } else {
        None
    }
}

fn is_short_alphanumeric_minified_name(name: &str) -> bool {
    name.len() <= 3
        && name.chars().all(|ch| ch.is_ascii_alphanumeric())
        && name.chars().any(|ch| ch.is_ascii_digit())
}
fn ts_function_matches_kind(function: &Function, kind: TsHelperKind) -> bool {
    match kind {
        TsHelperKind::Generator => ts_generator_state_function_matches(function),
        TsHelperKind::Values => ts_values_function_matches(function),
        TsHelperKind::AsyncValues => ts_async_values_function_matches(function),
        _ => false,
    }
}
fn ts_values_function_matches(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    ts_values_body_matches(function.params.len(), &body.stmts)
}
fn ts_async_values_function_matches(function: &Function) -> bool {
    let Some(body) = &function.body else {
        return false;
    };
    ts_async_values_body_matches(function.params.len(), &body.stmts)
}
/// `__asyncValues`: a single iterable param that reads `Symbol.asyncIterator`,
/// throws `TypeError` when the symbol is missing, and wraps a sync iterator in
/// a `Promise`-settling adapter (the nested `verb`/`settle` functions). The
/// `Promise` signal separates it from Babel's `_asyncIterator`, whose body
/// shares the first two signals and delegates wrapping to a separate helper.
/// Its sync fallback also reads `Symbol.iterator`, so `ts_values_body_matches`
/// excludes this shape.
fn ts_async_values_body_matches(param_len: usize, body: &[Stmt]) -> bool {
    if param_len != 1 {
        return false;
    }
    let own = collect_ts_helper_own_body_signals(body);
    own.symbol_async_iterator && own.type_error && collect_ts_helper_body_signals(body).promise
}
/// `__values` / `_ts_values`: a single iterable param, grabs `Symbol.iterator`,
/// and throws `TypeError` when the value is not iterable. Both signals sit in
/// the helper's own statements; a user function that merely contains an inlined
/// Babel iterable helper carries them only inside nested functions and is not
/// a helper (removing it would delete a live function).
fn ts_values_body_matches(param_len: usize, body: &[Stmt]) -> bool {
    if param_len != 1 {
        return false;
    }
    let signals = collect_ts_helper_own_body_signals(body);
    signals.symbol_iterator && signals.type_error && !signals.symbol_async_iterator
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
