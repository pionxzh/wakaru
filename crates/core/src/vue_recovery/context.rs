use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    Decl, Expr, FnDecl, ImportSpecifier, Module, ModuleDecl, ModuleItem, Pat, Stmt, VarDeclKind,
};

use super::helpers::{helper_name, VueHelper};
use super::syntax::{module_export_name, param_binding_ident, string_lit};
use super::VueRecoveryContext;

pub(super) fn collect_context(module: &Module, cm: Lrc<SourceMap>) -> VueRecoveryContext {
    let mut ctx = VueRecoveryContext {
        cm,
        ..Default::default()
    };
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) if import.src.value == *"vue" => {
                for specifier in &import.specifiers {
                    if let ImportSpecifier::Named(named) = specifier {
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

pub(super) fn collect_render_context(render: &FnDecl, ctx: &mut VueRecoveryContext) {
    let Some(body) = render.function.body.as_ref() else {
        return;
    };
    for stmt in &body.stmts {
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

pub(super) fn render_context_param(render: &FnDecl) -> Option<Atom> {
    render
        .function
        .params
        .first()
        .and_then(param_binding_ident)
        .map(|ident| ident.sym.clone())
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
