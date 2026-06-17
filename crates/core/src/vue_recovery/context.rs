use std::collections::HashMap;

use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, ExprOrSpread, ImportSpecifier, Lit, Module,
    ModuleDecl, ModuleItem, Pat, Stmt, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

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
                for specifier in &import.specifiers {
                    if let ImportSpecifier::Named(named) = specifier {
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
        RenderSource::Arrow(render) => render.body.visit_with(&mut inference),
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

pub(super) fn render_context_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::Function(render) => render
            .function
            .params
            .first()
            .and_then(param_binding_ident)
            .map(|ident| ident.sym.clone()),
        RenderSource::Arrow(render) => render.params.first().and_then(|param| match param {
            Pat::Ident(binding) => Some(binding.id.sym.clone()),
            _ => None,
        }),
    }
}

fn render_stmts(render: RenderSource<'_>) -> Option<&[Stmt]> {
    match render {
        RenderSource::Function(render) => render
            .function
            .body
            .as_ref()
            .map(|body| body.stmts.as_slice()),
        RenderSource::Arrow(render) => match render.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => Some(block.stmts.as_slice()),
            BlockStmtOrExpr::Expr(_) => None,
        },
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
