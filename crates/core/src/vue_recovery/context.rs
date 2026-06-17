use std::collections::HashMap;

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, ExprOrSpread, IfStmt, ImportSpecifier, Lit,
    MemberExpr, Module, ModuleDecl, ModuleItem, Pat, ReturnStmt, Stmt, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::expressions::{clean_expr, print_expr};
use super::helpers::{helper_name, VueHelper};
use super::syntax::{module_export_name, param_binding_ident, string_lit, wtf8_to_string};
use super::{RenderSource, VueRecoveryContext};

pub(super) fn collect_context(module: &Module, cm: Lrc<SourceMap>) -> VueRecoveryContext {
    let mut ctx = VueRecoveryContext {
        cm,
        ..Default::default()
    };
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
                                    .insert(named.local.sym.clone(), component.clone());
                            }
                            if source != "vue" {
                                if source.contains("vue") {
                                    ctx.vue_helper_candidates.insert(named.local.sym.clone());
                                }
                                continue;
                            }
                            let imported = named
                                .imported
                                .as_ref()
                                .map(module_export_name)
                                .unwrap_or_else(|| named.local.sym.to_string());
                            ctx.vue_helpers.insert(
                                named.local.sym.clone(),
                                VueHelper::from_imported_name(imported),
                            );
                        }
                        ImportSpecifier::Default(default) => {
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .insert(default.local.sym.clone(), component.clone());
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
                            if let Some(component) = &imported_component {
                                ctx.component_bindings
                                    .insert(namespace.local.sym.clone(), component.clone());
                            }
                        }
                    }
                }
            }
            ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) => {
                if !matches!(var.kind, VarDeclKind::Const | VarDeclKind::Var) {
                    continue;
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
                    if binding.id.sym.as_ref() == "__sfc__" {
                        if let Expr::Object(object) = init {
                            ctx.component_options = Some(object.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    ctx
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

pub(super) fn infer_render_helpers(render: RenderSource<'_>, ctx: &mut VueRecoveryContext) {
    if ctx.vue_helper_candidates.is_empty() {
        return;
    }

    let mut inference = HelperInference {
        candidates: &ctx.vue_helper_candidates,
        inferred: HashMap::new(),
    };
    match render {
        RenderSource::Function(render) => {
            if let Some(body) = render.function.body.as_ref() {
                body.visit_with(&mut inference);
            }
        }
        RenderSource::SetupArrow { render, .. } => render.body.visit_with(&mut inference),
    }

    for (local, helper) in inference.inferred {
        ctx.vue_helpers.entry(local).or_insert(helper);
    }
}

struct HelperInference<'a> {
    candidates: &'a std::collections::HashSet<Atom>,
    inferred: HashMap<Atom, VueHelper>,
}

impl Visit for HelperInference<'_> {
    fn visit_if_stmt(&mut self, if_stmt: &IfStmt) {
        self.infer_unref_expr(if_stmt.test.as_ref());
        if_stmt.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.infer_unref_expr(member.obj.as_ref());
        member.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(callee) = call_callee_ident(call) {
            if self.candidates.contains(&callee.sym) {
                if let Some(helper) = infer_call_helper(call) {
                    self.inferred.entry(callee.sym.clone()).or_insert(helper);
                }
            }
        }

        if let Some(VueHelper::CreateElementBlock | VueHelper::CreateElementVNode) =
            call_callee_ident(call).and_then(|callee| self.inferred.get(&callee.sym))
        {
            if let Some(fragment) = call
                .args
                .first()
                .and_then(|arg| ident_expr(arg.expr.as_ref()))
                .filter(|ident| self.candidates.contains(&ident.sym))
            {
                self.inferred
                    .entry(fragment.sym.clone())
                    .or_insert(VueHelper::Fragment);
            }
        }

        call.visit_children_with(self);
    }
}

impl HelperInference<'_> {
    fn infer_unref_expr(&mut self, expr: &Expr) {
        let Expr::Call(call) = unwrap_paren_expr(expr) else {
            return;
        };
        if !is_display_string_call(&call.args) {
            return;
        }
        let Some(callee) = call_callee_ident(call) else {
            return;
        };
        if !self.candidates.contains(&callee.sym) {
            return;
        }
        self.inferred.insert(callee.sym.clone(), VueHelper::Unref);
    }
}

fn unwrap_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unwrap_paren_expr(paren.expr.as_ref()),
        _ => expr,
    }
}

fn infer_call_helper(call: &CallExpr) -> Option<VueHelper> {
    if is_with_directives_call(&call.args) {
        return Some(VueHelper::WithDirectives);
    }
    if is_with_memo_call(&call.args) {
        return Some(VueHelper::WithMemo);
    }
    if is_create_slots_call(&call.args) {
        return Some(VueHelper::CreateSlots);
    }
    if is_render_list_call(&call.args) {
        return Some(VueHelper::RenderList);
    }
    if is_with_ctx_call(&call.args) {
        return Some(VueHelper::WithCtx);
    }
    if is_create_static_vnode_call(&call.args) {
        return Some(VueHelper::CreateStaticVNode);
    }
    if is_create_comment_vnode_call(&call.args) {
        return Some(VueHelper::CreateCommentVNode);
    }
    if is_element_vnode_call(&call.args) {
        return Some(VueHelper::CreateElementBlock);
    }
    if is_component_vnode_call(&call.args) {
        return Some(VueHelper::CreateVNode);
    }
    if is_resolve_component_call(&call.args) {
        return Some(VueHelper::ResolveComponent);
    }
    if is_display_string_call(&call.args) {
        return Some(VueHelper::ToDisplayString);
    }
    if is_open_block_call(&call.args) {
        return Some(VueHelper::OpenBlock);
    }
    None
}

fn is_with_directives_call(args: &[ExprOrSpread]) -> bool {
    matches!(args.get(1).map(|arg| arg.expr.as_ref()), Some(Expr::Array(array)) if array.elems.iter().flatten().any(|elem| matches!(elem.expr.as_ref(), Expr::Array(_))))
}

fn is_with_memo_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 4
        && matches!(
            args.get(1).map(|arg| arg.expr.as_ref()),
            Some(Expr::Arrow(_))
        )
}

fn is_create_slots_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Object(_))
    ) && matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Array(_))
    )
}

fn is_render_list_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_with_ctx_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Arrow(_))
    )
}

fn is_create_static_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Str(str))) if wtf8_to_string(&str.value).contains('<')
    )
}

fn is_create_comment_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        (
            args.first().map(|arg| arg.expr.as_ref()),
            args.get(1).map(|arg| arg.expr.as_ref())
        ),
        (Some(Expr::Lit(Lit::Str(_))), Some(Expr::Lit(Lit::Bool(_))))
    )
}

fn is_element_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.first().map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Str(str))) if !wtf8_to_string(&str.value).contains('<')
    ) && args.len() >= 2
}

fn is_component_vnode_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && !matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)) | Expr::Object(_))
        )
}

fn is_resolve_component_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)))
        )
}

fn is_display_string_call(args: &[ExprOrSpread]) -> bool {
    args.len() == 1
        && !matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Str(_)))
        )
}

fn is_open_block_call(args: &[ExprOrSpread]) -> bool {
    args.is_empty()
        || matches!(
            args.first().map(|arg| arg.expr.as_ref()),
            Some(Expr::Lit(Lit::Bool(_)))
        )
}

fn call_callee_ident(call: &CallExpr) -> Option<&swc_core::ecma::ast::Ident> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    ident_expr(callee.as_ref())
}

fn ident_expr(expr: &Expr) -> Option<&swc_core::ecma::ast::Ident> {
    match expr {
        Expr::Ident(ident) => Some(ident),
        _ => None,
    }
}

pub(super) fn collect_render_context(render: RenderSource<'_>, ctx: &mut VueRecoveryContext) {
    let Some(stmts) = render_stmts(render) else {
        return;
    };
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if let Some(component) = resolve_component_name(init, ctx) {
                ctx.component_bindings
                    .insert(binding.id.sym.clone(), component);
            }
            if let Some(directive) = resolve_directive_name(init, ctx) {
                ctx.directive_bindings
                    .insert(binding.id.sym.clone(), directive);
            }
        }
    }
}

pub(super) fn collect_setup_context(
    render: RenderSource<'_>,
    ctx: &mut VueRecoveryContext,
) -> Result<()> {
    let RenderSource::SetupArrow { setup_stmts, .. } = render else {
        return Ok(());
    };

    for stmt in setup_stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            let Some(value_expr) = computed_value_expr(init, ctx) else {
                continue;
            };
            let value = clean_expr(&print_expr(value_expr, ctx)?, ctx);
            ctx.setup_value_bindings
                .insert(binding.id.sym.clone(), value);
        }
    }

    Ok(())
}

pub(super) fn render_context_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::Function(render) => render
            .function
            .params
            .first()
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::SetupArrow { render, .. } => {
            render.params.first().and_then(|param| match param {
                Pat::Ident(binding) => Some(binding.id.sym.clone()),
                _ => None,
            })
        }
    }
}

fn render_stmts(render: RenderSource<'_>) -> Option<&[Stmt]> {
    match render {
        RenderSource::Function(render) => render
            .function
            .body
            .as_ref()
            .map(|body| body.stmts.as_slice()),
        RenderSource::SetupArrow { render, .. } => match render.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => Some(block.stmts.as_slice()),
            BlockStmtOrExpr::Expr(_) => None,
        },
    }
}

fn computed_value_expr<'a>(expr: &'a Expr, ctx: &VueRecoveryContext) -> Option<&'a Expr> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };
    if !is_computed_call(call, ctx) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| arrow_body_expr(arg.expr.as_ref()))
}

fn is_computed_call(call: &CallExpr, ctx: &VueRecoveryContext) -> bool {
    if helper_name(&call.callee, ctx) == Some(VueHelper::Computed) {
        return true;
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn arrow_body_expr(expr: &Expr) -> Option<&Expr> {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return None;
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(expr) => Some(expr.as_ref()),
        BlockStmtOrExpr::BlockStmt(block) => block.stmts.iter().rev().find_map(|stmt| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some(expr.as_ref()),
            _ => None,
        }),
    }
}

fn resolve_component_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveComponent) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}

pub(super) fn resolve_directive_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::ResolveDirective) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| string_lit(arg.expr.as_ref()))
}
