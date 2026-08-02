use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, AssignExpr, AssignTarget, BinaryOp, BlockStmtOrExpr, CallExpr, Callee,
    CatchClause, ClassDecl, CondExpr, Decl, ExportDecl, ExportSpecifier, Expr, ExprOrSpread,
    FnDecl, Function, Ident, IfStmt, ImportSpecifier, Lit, MemberExpr, MemberProp, Module,
    ModuleDecl, ModuleItem, ObjectLit, ObjectPat, ObjectPatProp, Param, ParenExpr, Pat, Prop,
    PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, UnaryOp, UpdateExpr, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::expressions::{
    clean_expr, clean_setup_stmt, clean_setup_stmt_preserving_ref_values, print_clean_setup_stmt,
    print_expr,
};
use super::helpers::{helper_name, VueHelper};
use super::imports;
use super::locals::{
    VueSetupLocalBinding, VueSetupRefBinding, VueSetupScriptBinding, VueSetupValueBinding,
};
use super::script_imports::VueScriptImport;
use super::setup_bindings::component_prop_names;
use super::slots::slot_call_binding;
use super::syntax::{
    module_export_name, param_binding_ident, prop_name, string_lit, wtf8_to_string,
};
use super::{
    RenderSource, VueRecoveryContext, VueRenderChildListBinding, VueRenderChildListSource,
    VueRenderSlotBinding,
};
use crate::js_names::{is_likely_generated_alias, is_valid_identifier_name};
use crate::rules::rename_utils::{rename_bindings, BindingRename};

const MAX_INLINE_COMPUTED_TEMPLATE_BINDING_LEN: usize = 80;

struct SetupLocalCandidate {
    bindings: Vec<Atom>,
    stmt: Stmt,
    template_selectable: bool,
    setup_order: usize,
    always_emit: bool,
    preserve_ref_values: bool,
}

#[derive(Clone, Copy)]
pub(super) struct CompiledScriptSetup<'a> {
    pub(super) setup_stmts: &'a [Stmt],
    pub(super) setup_props: Option<&'a Ident>,
    pub(super) setup_props_ctxt: Option<SyntaxContext>,
    pub(super) setup_context: Option<&'a Ident>,
    pub(super) setup_emit: Option<&'a Ident>,
    pub(super) setup_slots: Option<&'a Ident>,
    pub(super) setup_expose: Option<&'a Ident>,
}

pub(super) fn compiled_script_setup(
    options: Option<&ObjectLit>,
) -> Option<CompiledScriptSetup<'_>> {
    let options = options?;
    for prop in &options.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::Method(method) if prop_name(&method.key).as_deref() == Some("setup") => {
                if let Some(setup) = compiled_script_setup_from_function(&method.function) {
                    return Some(setup);
                }
            }
            Prop::KeyValue(key_value) if prop_name(&key_value.key).as_deref() == Some("setup") => {
                match unwrap_paren_expr(key_value.value.as_ref()) {
                    Expr::Fn(function) => {
                        if let Some(setup) = compiled_script_setup_from_function(&function.function)
                        {
                            return Some(setup);
                        }
                    }
                    Expr::Arrow(arrow) => {
                        if let Some(setup) = compiled_script_setup_from_arrow(arrow) {
                            return Some(setup);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    None
}

fn compiled_script_setup_from_function(function: &Function) -> Option<CompiledScriptSetup<'_>> {
    let body = function.body.as_ref()?;
    if !has_script_setup_marker(body) {
        return None;
    }
    let setup_props = function.params.first().and_then(param_binding_ident);
    let setup_context = function
        .params
        .get(1)
        .and_then(|param| pat_binding_ident(&param.pat));
    let setup_emit = function
        .params
        .get(1)
        .and_then(|param| named_object_pat_binding(&param.pat, "emit"));
    let setup_slots = function
        .params
        .get(1)
        .and_then(|param| named_object_pat_binding(&param.pat, "slots"));
    let setup_expose = function
        .params
        .get(1)
        .and_then(|param| named_object_pat_binding(&param.pat, "expose"));
    Some(CompiledScriptSetup {
        setup_stmts: body.stmts.as_slice(),
        setup_props,
        setup_props_ctxt: setup_props.map(|ident| ident.ctxt),
        setup_context,
        setup_emit,
        setup_slots,
        setup_expose,
    })
}

fn compiled_script_setup_from_arrow(arrow: &ArrowExpr) -> Option<CompiledScriptSetup<'_>> {
    let BlockStmtOrExpr::BlockStmt(body) = arrow.body.as_ref() else {
        return None;
    };
    if !has_script_setup_marker(body) {
        return None;
    }
    let setup_props = arrow.params.first().and_then(pat_binding_ident);
    let setup_context = arrow.params.get(1).and_then(pat_binding_ident);
    let setup_emit = arrow
        .params
        .get(1)
        .and_then(|pat| named_object_pat_binding(pat, "emit"));
    let setup_slots = arrow
        .params
        .get(1)
        .and_then(|pat| named_object_pat_binding(pat, "slots"));
    let setup_expose = arrow
        .params
        .get(1)
        .and_then(|pat| named_object_pat_binding(pat, "expose"));
    Some(CompiledScriptSetup {
        setup_stmts: body.stmts.as_slice(),
        setup_props,
        setup_props_ctxt: setup_props.map(|ident| ident.ctxt),
        setup_context,
        setup_emit,
        setup_slots,
        setup_expose,
    })
}

fn pat_binding_ident(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id),
        _ => None,
    }
}

fn named_object_pat_binding<'a>(pat: &'a Pat, name: &str) -> Option<&'a Ident> {
    let Pat::Object(object) = pat else {
        return None;
    };
    object.props.iter().find_map(|prop| match prop {
        ObjectPatProp::KeyValue(key_value)
            if prop_name(&key_value.key).as_deref() == Some(name) =>
        {
            pat_binding_ident(key_value.value.as_ref())
        }
        ObjectPatProp::Assign(assign) if assign.key.sym.as_ref() == name => Some(&assign.key),
        _ => None,
    })
}

fn has_script_setup_marker(body: &swc_core::ecma::ast::BlockStmt) -> bool {
    struct MarkerFinder(bool);

    impl Visit for MarkerFinder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if script_setup_marker_target(call).is_some() {
                self.0 = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = MarkerFinder(false);
    body.visit_with(&mut finder);
    finder.0
}

fn is_compiled_setup_artifact_stmt(
    stmt: &Stmt,
    setup_expose: Option<&(Atom, SyntaxContext)>,
) -> bool {
    if let (Stmt::Expr(expr_stmt), Some((expose_name, expose_ctxt))) = (stmt, setup_expose) {
        if let Expr::Call(call) = expr_stmt.expr.as_ref() {
            if call.args.is_empty()
                && matches!(&call.callee, Callee::Expr(callee) if matches!(callee.as_ref(), Expr::Ident(ident) if ident.sym == *expose_name && ident.ctxt == *expose_ctxt))
            {
                return true;
            }
        }
    }

    matches!(stmt, Stmt::Expr(expr_stmt) if matches!(unwrap_paren_expr(expr_stmt.expr.as_ref()), Expr::Call(call) if script_setup_marker_target(call).is_some()))
}

fn compiled_setup_return_binding(stmts: &[Stmt]) -> Option<(Atom, SyntaxContext)> {
    stmts.iter().find_map(|stmt| {
        let Stmt::Expr(expr_stmt) = stmt else {
            return None;
        };
        let Expr::Call(call) = unwrap_paren_expr(expr_stmt.expr.as_ref()) else {
            return None;
        };
        script_setup_marker_target(call).map(|ident| (ident.sym.clone(), ident.ctxt))
    })
}

fn compiled_setup_return_values(
    stmts: &[Stmt],
    return_binding: &(Atom, SyntaxContext),
) -> Vec<(Atom, Box<Expr>, usize)> {
    stmts
        .iter()
        .enumerate()
        .filter_map(|stmt| match stmt {
            (setup_order, Stmt::Decl(Decl::Var(var))) => Some((setup_order, var.decls.as_slice())),
            _ => None,
        })
        .find_map(|(setup_order, decls)| {
            decls.iter().find_map(|decl| {
                let Pat::Ident(binding) = &decl.name else {
                    return None;
                };
                if binding.id.sym != return_binding.0 || binding.id.ctxt != return_binding.1 {
                    return None;
                }
                let Expr::Object(object) = decl.init.as_deref()? else {
                    return None;
                };
                Some((object, setup_order))
            })
        })
        .map(|(object, setup_order)| {
            object
                .props
                .iter()
                .filter_map(|prop| {
                    let PropOrSpread::Prop(prop) = prop else {
                        return None;
                    };
                    let Prop::KeyValue(key_value) = prop.as_ref() else {
                        return None;
                    };
                    let name = prop_name(&key_value.key)?;
                    is_valid_identifier_name(name.as_ref())
                        .then(|| (Atom::from(name), key_value.value.clone(), setup_order))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn is_undefined_placeholder(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => ident.sym.as_ref() == "undefined",
        Expr::Unary(unary) => unary.op == UnaryOp::Void,
        _ => false,
    }
}

fn script_setup_marker_target(call: &CallExpr) -> Option<&Ident> {
    if call.args.len() != 3 || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    if !matches!(member.obj.as_ref(), Expr::Ident(object) if object.sym.as_ref() == "Object") {
        return None;
    }
    if !matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "defineProperty") {
        return None;
    }
    let has_marker_name = call.args.get(1).is_some_and(
        |arg| matches!(arg.expr.as_ref(), Expr::Lit(Lit::Str(value)) if wtf8_to_string(&value.value) == "__isScriptSetup"),
    );
    if !has_marker_name {
        return None;
    }
    let Expr::Object(descriptor) = call.args.get(2)?.expr.as_ref() else {
        return None;
    };
    if !object_bool_prop(descriptor, "enumerable", false)
        || !object_bool_prop(descriptor, "value", true)
    {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| ident_expr(unwrap_paren_expr(arg.expr.as_ref())))
}

fn object_bool_prop(object: &ObjectLit, name: &str, expected: bool) -> bool {
    object.props.iter().any(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return false;
        };
        matches!(
            prop.as_ref(),
            Prop::KeyValue(key_value)
                if prop_name(&key_value.key).as_deref() == Some(name)
                    && matches!(key_value.value.as_ref(), Expr::Lit(Lit::Bool(value)) if value.value == expected)
        )
    })
}

/// Record the resolved `SyntaxContext` of every top-level binding (imports plus
/// `var`/`fn`/`class` declarations). Helper recognition uses it to tell a
/// genuine reference to an imported Vue helper apart from an inner-scope local
/// that reuses the (minified) name; alias renaming uses it to build
/// `SyntaxContext`-keyed `BindingRename`s for `rename_utils::BindingRenamer`.
fn top_level_binding_ctxts(module: &Module) -> HashMap<Atom, SyntaxContext> {
    let mut ctxts = HashMap::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                for specifier in &import.specifiers {
                    let local = match specifier {
                        ImportSpecifier::Named(named) => &named.local,
                        ImportSpecifier::Default(default) => &default.local,
                        ImportSpecifier::Namespace(namespace) => &namespace.local,
                    };
                    ctxts.insert(local.sym.clone(), local.ctxt);
                }
            }
            ModuleItem::Stmt(Stmt::Decl(decl))
            | ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl { decl, .. })) => {
                record_decl_binding_ctxts(decl, &mut ctxts);
            }
            _ => {}
        }
    }
    ctxts
}

fn record_decl_binding_ctxts(decl: &Decl, ctxts: &mut HashMap<Atom, SyntaxContext>) {
    match decl {
        Decl::Fn(function) => {
            ctxts.insert(function.ident.sym.clone(), function.ident.ctxt);
        }
        Decl::Class(class) => {
            ctxts.insert(class.ident.sym.clone(), class.ident.ctxt);
        }
        Decl::Var(var) => {
            for declarator in &var.decls {
                record_pat_binding_ctxts(&declarator.name, ctxts);
            }
        }
        _ => {}
    }
}

fn record_pat_binding_ctxts(pat: &Pat, ctxts: &mut HashMap<Atom, SyntaxContext>) {
    match pat {
        Pat::Ident(binding) => {
            ctxts.insert(binding.id.sym.clone(), binding.id.ctxt);
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                record_pat_binding_ctxts(elem, ctxts);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        record_pat_binding_ctxts(&key_value.value, ctxts)
                    }
                    ObjectPatProp::Assign(assign) => {
                        ctxts.insert(assign.key.id.sym.clone(), assign.key.id.ctxt);
                    }
                    ObjectPatProp::Rest(rest) => record_pat_binding_ctxts(&rest.arg, ctxts),
                }
            }
        }
        Pat::Rest(rest) => record_pat_binding_ctxts(&rest.arg, ctxts),
        Pat::Assign(assign) => record_pat_binding_ctxts(&assign.left, ctxts),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

/// Collect each pattern-bound ident as a `(name, SyntaxContext)` pair. Mirrors
/// [`record_pat_binding_ctxts`] but keeps distinct contexts for the same name so
/// candidate sets can be matched against a resolved reference's own binding
/// identity rather than by name alone.
fn collect_pat_binding_idents(pat: &Pat, bindings: &mut HashSet<(Atom, SyntaxContext)>) {
    match pat {
        Pat::Ident(binding) => {
            bindings.insert((binding.id.sym.clone(), binding.id.ctxt));
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_binding_idents(elem, bindings);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_pat_binding_idents(&key_value.value, bindings)
                    }
                    ObjectPatProp::Assign(assign) => {
                        bindings.insert((assign.key.id.sym.clone(), assign.key.id.ctxt));
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_binding_idents(&rest.arg, bindings),
                }
            }
        }
        Pat::Rest(rest) => collect_pat_binding_idents(&rest.arg, bindings),
        Pat::Assign(assign) => collect_pat_binding_idents(&assign.left, bindings),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

pub(super) fn collect_context(
    module: &Module,
    cm: Lrc<SourceMap>,
    component_bindings: HashMap<Atom, String>,
    imported_composable_ref_props: HashMap<Atom, HashSet<Atom>>,
) -> VueRecoveryContext {
    let default_exported_bindings = default_exported_bindings(module);
    let mut ctx = VueRecoveryContext {
        cm,
        component_bindings,
        imported_composable_ref_props,
        ..Default::default()
    };
    ctx.top_level_binding_ctxts = top_level_binding_ctxts(module);
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) => {
                let source = wtf8_to_string(&import.src.value);
                let imported_component = vue_component_name_from_source(&source);
                for specifier in &import.specifiers {
                    match specifier {
                        ImportSpecifier::Named(named) => {
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(named.local.sym.clone())
                                    .or_insert_with(|| {
                                        preferred_imported_component_name(
                                            &named.local.sym,
                                            component,
                                        )
                                    });
                            }
                            let imported = named
                                .imported
                                .as_ref()
                                .map(module_export_name)
                                .unwrap_or_else(|| named.local.sym.to_string());
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    named.local.sym.clone(),
                                    VueScriptImport::Named {
                                        source: source.clone(),
                                        imported: imported.clone(),
                                    },
                                );
                            }
                            if source == "pinia" && imported == "storeToRefs" {
                                ctx.vue_helpers
                                    .insert(named.local.sym.clone(), VueHelper::Other(imported));
                                continue;
                            }
                            if source != "vue" {
                                if is_vue_helper_candidate_source(&source) {
                                    ctx.vue_helper_candidates.insert(named.local.sym.clone());
                                }
                                continue;
                            }
                            ctx.vue_helpers.insert(
                                named.local.sym.clone(),
                                VueHelper::from_imported_name(imported),
                            );
                        }
                        ImportSpecifier::Default(default) => {
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    default.local.sym.clone(),
                                    VueScriptImport::Default {
                                        source: source.clone(),
                                    },
                                );
                            }
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(default.local.sym.clone())
                                    .or_insert_with(|| {
                                        preferred_imported_component_name(
                                            &default.local.sym,
                                            component,
                                        )
                                    });
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            if source == "vue" || is_vue_helper_candidate_source(&source) {
                                ctx.vue_namespaces.insert(namespace.local.sym.clone());
                            }
                            if source != "vue" {
                                ctx.script_imports.insert(
                                    namespace.local.sym.clone(),
                                    VueScriptImport::Namespace {
                                        source: source.clone(),
                                    },
                                );
                            }
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .entry(namespace.local.sym.clone())
                                    .or_insert_with(|| {
                                        preferred_imported_component_name(
                                            &namespace.local.sym,
                                            component,
                                        )
                                    });
                            }
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                collect_var_decl_context(var, &mut ctx, &default_exported_bindings);
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) => {
                collect_fn_decl_context(function, &mut ctx);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => match &export.decl {
                Decl::Var(var) => {
                    collect_var_decl_context(var, &mut ctx, &default_exported_bindings);
                }
                Decl::Fn(function) => collect_fn_decl_context(function, &mut ctx),
                _ => {}
            },
            _ => {}
        }
    }
    ctx
}

fn collect_fn_decl_context(function: &FnDecl, ctx: &mut VueRecoveryContext) {
    if is_slot_result_normalizer_function(&function.function) {
        ctx.slot_result_normalizers
            .insert(function.ident.sym.clone());
    }
}

fn is_slot_result_normalizer_function(function: &Function) -> bool {
    let Some(param) = function.params.first() else {
        return false;
    };
    if function.params.len() != 1 {
        return false;
    }
    let Pat::Ident(param) = &param.pat else {
        return false;
    };
    let Some(body) = &function.body else {
        return false;
    };
    let [Stmt::If(if_stmt), Stmt::Return(final_return)] = body.stmts.as_slice() else {
        return false;
    };
    if !is_length_one_test(if_stmt.test.as_ref(), &param.id.sym) {
        return false;
    }
    if !if_stmt
        .cons
        .as_ref()
        .is_return_with(|expr| is_member_index_expr(expr, &param.id.sym, 0.0))
    {
        return false;
    }
    final_return
        .arg
        .as_deref()
        .is_some_and(|expr| is_ident_expr(expr, &param.id.sym))
}

trait ReturnStmtExt {
    fn is_return_with(&self, predicate: impl FnOnce(&Expr) -> bool) -> bool;
}

impl ReturnStmtExt for Stmt {
    fn is_return_with(&self, predicate: impl FnOnce(&Expr) -> bool) -> bool {
        match self {
            Stmt::Return(return_stmt) => return_stmt.arg.as_deref().is_some_and(predicate),
            Stmt::Block(block) => match block.stmts.as_slice() {
                [Stmt::Return(return_stmt)] => return_stmt.arg.as_deref().is_some_and(predicate),
                _ => false,
            },
            _ => false,
        }
    }
}

fn is_length_one_test(expr: &Expr, param: &Atom) -> bool {
    let Expr::Bin(bin) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
        return false;
    }
    (is_member_prop_expr(bin.left.as_ref(), param, "length")
        && is_number_lit(bin.right.as_ref(), 1.0))
        || (is_member_prop_expr(bin.right.as_ref(), param, "length")
            && is_number_lit(bin.left.as_ref(), 1.0))
}

fn is_member_prop_expr(expr: &Expr, object: &Atom, prop: &str) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    is_ident_expr(member.obj.as_ref(), object) && member_prop_is_named(&member.prop, prop)
}

fn is_member_index_expr(expr: &Expr, object: &Atom, index: f64) -> bool {
    let Expr::Member(member) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !is_ident_expr(member.obj.as_ref(), object) {
        return false;
    }
    let MemberProp::Computed(computed) = &member.prop else {
        return false;
    };
    is_number_lit(computed.expr.as_ref(), index)
}

fn is_ident_expr(expr: &Expr, sym: &Atom) -> bool {
    matches!(unwrap_paren_expr(expr), Expr::Ident(ident) if &ident.sym == sym)
}

fn is_number_lit(expr: &Expr, value: f64) -> bool {
    matches!(unwrap_paren_expr(expr), Expr::Lit(Lit::Num(number)) if number.value == value)
}

fn default_exported_bindings(module: &Module) -> HashSet<Atom> {
    let mut bindings = HashSet::new();

    for item in &module.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDefaultExpr(export) => {
                if let Expr::Ident(ident) = export.expr.as_ref() {
                    bindings.insert(ident.sym.clone());
                }
            }
            ModuleDecl::ExportNamed(export) if export.src.is_none() => {
                for specifier in &export.specifiers {
                    let ExportSpecifier::Named(named) = specifier else {
                        continue;
                    };
                    let exported = named
                        .exported
                        .as_ref()
                        .map(module_export_name)
                        .unwrap_or_else(|| module_export_name(&named.orig));
                    if exported == "default" {
                        bindings.insert(Atom::from(module_export_name(&named.orig)));
                    }
                }
            }
            _ => {}
        }
    }

    bindings
}

pub(super) fn collect_script_local_context(
    module: &Module,
    ctx: &mut VueRecoveryContext,
) -> Result<()> {
    let reserved_bindings = script_local_reserved_bindings(module, ctx);
    let mut used_bindings = reserved_bindings.clone();
    used_bindings.extend(ctx.script_imports.keys().cloned());

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => {
                collect_script_local_decl(decl, ctx, &reserved_bindings, &mut used_bindings)?
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_script_local_decl(
                    &export.decl,
                    ctx,
                    &reserved_bindings,
                    &mut used_bindings,
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn script_local_reserved_bindings(module: &Module, ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut reserved = HashSet::new();
    reserved.extend(
        ctx.setup_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(
        ctx.setup_local_bindings
            .iter()
            .flat_map(|binding| binding.emitted_bindings.iter().cloned()),
    );
    reserved.extend(
        ctx.setup_ref_script_bindings
            .iter()
            .map(|binding| binding.binding.clone()),
    );
    reserved.extend(ctx.bindings.values.keys().cloned());

    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => collect_decl_bindings(decl, &mut reserved),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_decl_bindings(&export.decl, &mut reserved);
            }
            _ => {}
        }
    }
    reserved
}

fn collect_decl_bindings(decl: &Decl, bindings: &mut HashSet<Atom>) {
    match decl {
        Decl::Fn(function) => {
            bindings.insert(function.ident.sym.clone());
        }
        Decl::Class(class) => {
            bindings.insert(class.ident.sym.clone());
        }
        Decl::Var(var) => {
            for declarator in &var.decls {
                collect_pat_bindings(&declarator.name, bindings);
            }
        }
        _ => {}
    }
}

fn emitted_stmt_bindings(source: &str, ctx: &VueRecoveryContext, fallback: &[Atom]) -> Vec<Atom> {
    let bindings = emitted_decl_bindings(source, ctx);
    if bindings.is_empty() {
        fallback.to_vec()
    } else {
        bindings
    }
}

fn emitted_decl_bindings(source: &str, ctx: &VueRecoveryContext) -> Vec<Atom> {
    let Ok(module) = super::parse_module(source, ctx.cm.clone()) else {
        return Vec::new();
    };

    let mut bindings = HashSet::new();
    for item in &module.body {
        match item {
            ModuleItem::Stmt(Stmt::Decl(decl)) => collect_decl_bindings(decl, &mut bindings),
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                collect_decl_bindings(&export.decl, &mut bindings);
            }
            _ => {}
        }
    }

    let mut bindings = bindings.into_iter().collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
    bindings.dedup();
    bindings
}

fn collect_script_local_decl(
    decl: &Decl,
    ctx: &mut VueRecoveryContext,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> Result<()> {
    match decl {
        Decl::Fn(function) => push_script_local_binding(
            ctx,
            vec![function.ident.sym.clone()],
            Stmt::Decl(Decl::Fn(function.clone())),
            reserved_bindings,
            used_bindings,
        ),
        Decl::Class(class) => push_script_local_binding(
            ctx,
            vec![class.ident.sym.clone()],
            Stmt::Decl(Decl::Class(class.clone())),
            reserved_bindings,
            used_bindings,
        ),
        Decl::Var(var) => {
            for declarator in &var.decls {
                if declarator.init.as_deref().is_some_and(|init| {
                    component_name_from_init(init, &ctx.component_bindings).is_some()
                }) {
                    continue;
                }
                let mut bindings = HashSet::new();
                collect_pat_bindings(&declarator.name, &mut bindings);
                if bindings.is_empty() {
                    continue;
                }
                let mut single_var = var.as_ref().clone();
                single_var.decls = vec![declarator.clone()];
                let mut bindings = bindings.into_iter().collect::<Vec<_>>();
                bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
                push_script_local_binding(
                    ctx,
                    bindings,
                    Stmt::Decl(Decl::Var(Box::new(single_var))),
                    reserved_bindings,
                    used_bindings,
                )?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn push_script_local_binding(
    ctx: &mut VueRecoveryContext,
    bindings: Vec<Atom>,
    mut stmt: Stmt,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> Result<()> {
    let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
    let mut source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    if is_transpiler_runtime_helper_source(&source) {
        return Ok(());
    }

    let import_aliases = colliding_import_aliases(&stmt, ctx, reserved_bindings, used_bindings);
    if !import_aliases.is_empty() {
        rename_bindings(&mut stmt, &binding_renames(&import_aliases, ctx));
        let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
        source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    }

    if !source.is_empty() {
        let cleaned_stmt = clean_setup_stmt(&stmt, ctx);
        let emitted_bindings = emitted_stmt_bindings(&source, ctx, &bindings);
        ctx.script_local_bindings.push(VueSetupLocalBinding {
            bindings,
            emitted_bindings,
            refs: stmt_ident_refs(&cleaned_stmt),
            source,
            import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
            stmt: cleaned_stmt,
            module_scope: true,
            template_selectable: true,
            setup_order: 0,
            always_emit: false,
            preserve_ref_values: false,
        });
    }
    Ok(())
}

pub(super) fn render_local_declaration_with_aliases(
    ctx: &VueRecoveryContext,
    declaration: &VueSetupLocalBinding,
    aliases: &HashMap<Atom, Atom>,
    props_binding: Option<&str>,
) -> Result<VueSetupLocalBinding> {
    let mut stmt = declaration.stmt.clone();
    if declaration.module_scope && !aliases.is_empty() {
        rename_bindings(&mut stmt, &binding_renames(aliases, ctx));
    }

    let mut cleaned_stmt = if declaration.preserve_ref_values {
        clean_setup_stmt_preserving_ref_values(&stmt, ctx)
    } else {
        clean_setup_stmt(&stmt, ctx)
    };
    if !declaration.module_scope {
        if let Some(props_binding) = props_binding {
            rename_bindings(&mut cleaned_stmt, &setup_props_renames(ctx, props_binding));
        }
    }
    let source = print_clean_setup_stmt(&cleaned_stmt, ctx)?;
    let bindings = if declaration.module_scope {
        declaration
            .bindings
            .iter()
            .map(|binding| {
                aliases
                    .get(binding)
                    .cloned()
                    .unwrap_or_else(|| binding.clone())
            })
            .collect()
    } else {
        declaration.bindings.clone()
    };
    let emitted_bindings = emitted_stmt_bindings(&source, ctx, &bindings);

    Ok(VueSetupLocalBinding {
        bindings,
        emitted_bindings,
        refs: stmt_ident_refs(&cleaned_stmt),
        source,
        import_refs: stmt_import_refs(&cleaned_stmt, &ctx.script_imports),
        stmt: cleaned_stmt,
        module_scope: declaration.module_scope,
        template_selectable: declaration.template_selectable,
        setup_order: declaration.setup_order,
        always_emit: declaration.always_emit,
        preserve_ref_values: declaration.preserve_ref_values,
    })
}

/// Renames for the setup `props` sources — the setup parameter
/// (`setup_props_context`) and every `setup_props_aliases` entry — onto the
/// emitted `props_binding`, keyed on each source's recorded `(name, ctxt)`.
/// Replaces the former bespoke `SetupPropsRefRewriter` visitor; `BindingRenamer`
/// preserves context and expands `Prop::Shorthand`, and matches only the
/// resolved props binding, never an inner-scope local of the same name.
fn setup_props_renames(ctx: &VueRecoveryContext, props_binding: &str) -> Vec<BindingRename> {
    let new = Atom::from(props_binding.to_string());
    let mut renames = Vec::new();
    if let (Some(name), Some(ctxt)) = (&ctx.setup_props_context, ctx.setup_props_context_ctxt) {
        renames.push(BindingRename {
            old: (name.clone(), ctxt),
            new: new.clone(),
        });
    }
    for (alias, ctxt) in &ctx.setup_props_alias_ctxts {
        renames.push(BindingRename {
            old: (alias.clone(), *ctxt),
            new: new.clone(),
        });
    }
    renames
}

/// Convert a name-keyed alias map into `SyntaxContext`-keyed renames for
/// `rename_utils::BindingRenamer`, which rewrites both the declaration and every
/// resolved reference (and expands shorthand) without the hand-rolled scope
/// tracking the old `ImportAliasRenamer`/`rename_top_level_decl_bindings` needed.
/// Names without a recorded top-level context are skipped (they are not
/// top-level bindings, so the alias would not apply).
fn binding_renames(aliases: &HashMap<Atom, Atom>, ctx: &VueRecoveryContext) -> Vec<BindingRename> {
    aliases
        .iter()
        .filter_map(|(old, new)| {
            ctx.top_level_binding_ctxts
                .get(old)
                .map(|ctxt| BindingRename {
                    old: (old.clone(), *ctxt),
                    new: new.clone(),
                })
        })
        .collect()
}

/// Renames for the setup-scope aliases in `ctx.bindings.aliases`, keyed on the
/// recorded `(name, ctxt)` of each alias source so `BindingRenamer` rewrites only
/// the aliased binding's references and never an inner-scope local of the same
/// name. Replaces the former bespoke `SetupAliasCleaner` visitor. Falls back to a
/// top-level binding context for aliases that originate at module scope.
pub(super) fn setup_alias_renames(ctx: &VueRecoveryContext) -> Vec<BindingRename> {
    ctx.bindings
        .aliases
        .iter()
        .filter_map(|(from, to)| {
            ctx.bindings
                .alias_ctxts
                .get(from)
                .or_else(|| ctx.top_level_binding_ctxts.get(from))
                .map(|ctxt| BindingRename {
                    old: (from.clone(), *ctxt),
                    new: to.clone(),
                })
        })
        .collect()
}

fn is_transpiler_runtime_helper_source(source: &str) -> bool {
    source.contains("suspendedStart")
        && source.contains("_invoke")
        && (source.contains("@@iterator") || source.contains("__await"))
}

fn is_vue_helper_candidate_source(source: &str) -> bool {
    if source.contains("runtime-core") || source.contains("runtime-dom") {
        return true;
    }
    if is_vue_adjacent_package_source(source) {
        return false;
    }
    if is_bare_import_source(source) {
        return false;
    }
    source.contains("vue")
}

fn is_vue_adjacent_package_source(source: &str) -> bool {
    let source = source.to_ascii_lowercase();
    source.contains("vueuse")
        || source.contains("vue-router")
        || source.contains("vuex")
        || source.contains("vue-i18n")
        || source.contains("vue-demi")
        || source.contains("vue-query")
}

fn is_bare_import_source(source: &str) -> bool {
    !source.starts_with('.')
        && !source.starts_with('/')
        && !source.starts_with("file:")
        && !source.starts_with("http:")
        && !source.starts_with("https:")
}

fn colliding_import_aliases(
    stmt: &Stmt,
    ctx: &mut VueRecoveryContext,
    reserved_bindings: &HashSet<Atom>,
    used_bindings: &mut HashSet<Atom>,
) -> HashMap<Atom, Atom> {
    let import_refs = stmt_import_refs(stmt, &ctx.script_imports);
    let mut aliases = HashMap::new();
    for import_ref in import_refs {
        if !reserved_bindings.contains(&import_ref) {
            continue;
        }
        let Some(import) = ctx.script_imports.get(&import_ref).cloned() else {
            continue;
        };
        let alias = unique_script_import_alias(&import_ref, used_bindings);
        ctx.script_imports.insert(alias.clone(), import);
        aliases.insert(import_ref, alias);
    }
    aliases
}

fn unique_script_import_alias(binding: &Atom, used_bindings: &mut HashSet<Atom>) -> Atom {
    let mut index = 1;
    loop {
        let candidate = Atom::from(format!("{}_{index}", binding.as_ref()));
        if used_bindings.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn collect_var_decl_context(
    var: &VarDecl,
    ctx: &mut VueRecoveryContext,
    default_exported_bindings: &HashSet<Atom>,
) {
    if !matches!(var.kind, VarDeclKind::Const | VarDeclKind::Var) {
        return;
    }
    for decl in &var.decls {
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let Some(init) = decl.init.as_deref() else {
            continue;
        };
        if let Expr::Object(object) = init {
            ctx.object_bindings
                .insert(binding.id.sym.clone(), object.clone());
        }
        if is_vue_fragment_symbol_init(init) {
            ctx.vue_helpers
                .insert(binding.id.sym.clone(), VueHelper::Fragment);
        }
        if is_likely_vue_runtime_require_namespace(&binding.id.sym, init) {
            ctx.vue_namespaces.insert(binding.id.sym.clone());
        }
        if let Some(ref_props) = provider_ref_props_from_init(init, ctx) {
            ctx.provider_ref_bindings
                .insert(binding.id.sym.clone(), ref_props);
        }
        if let Some(component) = component_name_from_init(init, &ctx.component_bindings) {
            ctx.component_bindings
                .insert(binding.id.sym.clone(), component);
        }
        if binding.id.sym.as_ref() == "__sfc__"
            || default_exported_bindings.contains(&binding.id.sym)
        {
            if let Some(object) = component_options_from_init(init) {
                ctx.component_options = Some(object.clone());
            }
        }
    }
}

fn is_vue_fragment_symbol_init(expr: &Expr) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Member(member) = callee.as_ref() else {
        return false;
    };
    let Expr::Ident(object) = member.obj.as_ref() else {
        return false;
    };
    if object.sym.as_ref() != "Symbol" {
        return false;
    }
    let MemberProp::Ident(prop) = &member.prop else {
        return false;
    };
    if prop.sym.as_ref() != "for" {
        return false;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
        .as_deref()
        == Some("v-fgt")
}

fn is_likely_vue_runtime_require_namespace(binding: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    if !call_callee_ident(call)
        .is_some_and(|callee| matches!(callee.sym.as_ref(), "require" | "__webpack_require__"))
    {
        return false;
    }

    if let Some(source) = call
        .args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
    {
        return source == "vue"
            || source.contains("@vue/runtime")
            || source.contains("vue/dist")
            || source.contains("vue.runtime");
    }

    let binding = binding.to_string().to_ascii_lowercase();
    (binding.contains("vue") && binding.contains("runtime"))
        || binding.contains("vue__webpack_imported_module")
}

pub(super) fn component_options_from_init(expr: &Expr) -> Option<&ObjectLit> {
    match unwrap_paren_expr(expr) {
        Expr::Object(object) => Some(object),
        Expr::Call(call) => {
            call.args
                .first()
                .and_then(|arg| match unwrap_paren_expr(arg.expr.as_ref()) {
                    Expr::Object(object) => Some(object),
                    _ => None,
                })
        }
        _ => None,
    }
}

pub(super) fn component_name_from_init(
    expr: &Expr,
    component_bindings: &HashMap<Atom, String>,
) -> Option<String> {
    match unwrap_paren_expr(expr) {
        Expr::Object(object) => component_name_from_options(object),
        Expr::Call(call) => call.args.first().and_then(|arg| match arg.expr.as_ref() {
            Expr::Object(object) => component_name_from_options(object),
            Expr::Ident(ident) => component_bindings.get(&ident.sym).cloned(),
            Expr::Call(_) | Expr::Paren(_) => {
                component_name_from_init(arg.expr.as_ref(), component_bindings)
            }
            _ => None,
        }),
        _ => None,
    }
}

pub(super) fn component_name_from_options(object: &ObjectLit) -> Option<String> {
    object.props.iter().find_map(|prop| {
        let PropOrSpread::Prop(prop) = prop else {
            return None;
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return None;
        };
        matches!(
            prop_name(&key_value.key).as_deref(),
            Some("__name" | "name")
        )
        .then(|| string_lit(key_value.value.as_ref()))
        .flatten()
    })
}

fn vue_component_name_from_source(source: &str) -> Option<String> {
    let file = source
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
        .trim_start_matches("./");
    if source.contains(".vue") {
        let name = file.split(".vue").next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }

    let stem = file
        .strip_suffix(".mjs")
        .or_else(|| file.strip_suffix(".js"))?;
    let name = stem
        .split('-')
        .next()
        .unwrap_or(stem)
        .split('.')
        .next()
        .unwrap_or(stem);
    let starts_with_uppercase = name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase());
    (starts_with_uppercase && !name.is_empty()).then(|| name.to_string())
}

fn preferred_imported_component_name(local: &Atom, inferred: &str) -> String {
    let local = local.as_ref();
    let looks_authored = !is_likely_generated_alias(local)
        && local
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        && local
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '$');
    if looks_authored {
        local.to_string()
    } else {
        inferred.to_string()
    }
}

mod helper_inference;
mod setup_context;
mod setup_values;

pub(super) use helper_inference::{call_callee_ident, infer_render_helpers, unwrap_paren_expr};
pub(super) use setup_context::{
    collect_render_context, collect_setup_context, is_ref_object_alias, is_ref_object_expr,
};
pub(super) use setup_values::{
    render_context_param, render_props_context_param, render_setup_context_param,
    resolve_directive_name, stmt_ident_refs,
};

use helper_inference::*;
use setup_context::*;
use setup_values::*;

#[cfg(test)]
mod tests;
