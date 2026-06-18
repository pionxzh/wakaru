use std::collections::HashMap;

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap};
use swc_core::ecma::ast::{
    BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, ExprOrSpread, IfStmt, ImportSpecifier, Lit,
    MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, Pat, Prop, PropOrSpread,
    ReturnStmt, Stmt, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitWith};

use super::expressions::{clean_expr, print_expr};
use super::helpers::{helper_name, VueHelper};
use super::syntax::{
    module_export_name, param_binding_ident, prop_name, string_lit, wtf8_to_string,
};
use super::{RenderSource, VueRecoveryContext};

pub(super) fn collect_context(
    module: &Module,
    cm: Lrc<SourceMap>,
    component_bindings: HashMap<Atom, String>,
) -> VueRecoveryContext {
    let mut ctx = VueRecoveryContext {
        cm,
        component_bindings,
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
                collect_var_decl_context(var, &mut ctx);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
                if let Decl::Var(var) = &export.decl {
                    collect_var_decl_context(var, &mut ctx);
                }
            }
            _ => {}
        }
    }
    ctx
}

fn collect_var_decl_context(var: &VarDecl, ctx: &mut VueRecoveryContext) {
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
        if let Some(component) = component_name_from_init(init, &ctx.component_bindings) {
            ctx.component_bindings
                .insert(binding.id.sym.clone(), component);
        }
        if binding.id.sym.as_ref() == "__sfc__" {
            if let Expr::Object(object) = init {
                ctx.component_options = Some(object.clone());
            }
        }
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

fn component_name_from_options(object: &ObjectLit) -> Option<String> {
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
        if let Callee::Expr(callee) = &call.callee {
            self.infer_unref_expr(callee.as_ref());
        }

        if let Some((callee, fragment)) = self.fragment_block_call(call) {
            self.inferred
                .insert(callee.sym.clone(), VueHelper::CreateElementBlock);
            self.inferred
                .insert(fragment.sym.clone(), VueHelper::Fragment);
        }

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
    fn fragment_block_call<'a>(
        &self,
        call: &'a CallExpr,
    ) -> Option<(
        &'a swc_core::ecma::ast::Ident,
        &'a swc_core::ecma::ast::Ident,
    )> {
        let callee = call_callee_ident(call)?;
        if !self.candidates.contains(&callee.sym) {
            return None;
        }
        if !is_fragment_patch_flag(call.args.get(3).map(|arg| arg.expr.as_ref())) {
            return None;
        }
        let fragment = call
            .args
            .first()
            .and_then(|arg| ident_expr(arg.expr.as_ref()))?;
        if !self.candidates.contains(&fragment.sym) {
            return None;
        }
        Some((callee, fragment))
    }

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

fn is_fragment_patch_flag(expr: Option<&Expr>) -> bool {
    matches!(
        expr,
        Some(Expr::Lit(Lit::Num(number)))
            if matches!(number.value as i32, 64 | 128 | 256)
    )
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
    if is_render_slot_call(&call.args) {
        return Some(VueHelper::RenderSlot);
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
    if is_create_text_vnode_call(&call.args) {
        return Some(VueHelper::CreateTextVNode);
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

fn is_render_slot_call(args: &[ExprOrSpread]) -> bool {
    args.len() >= 2
        && args
            .first()
            .is_some_and(|arg| is_slots_source_expr(arg.expr.as_ref()))
}

fn is_slots_source_expr(expr: &Expr) -> bool {
    match unwrap_paren_expr(expr) {
        Expr::Ident(ident) => matches!(ident.sym.as_ref(), "$slots" | "slots"),
        Expr::Member(member) => is_slots_member_prop(&member.prop),
        _ => false,
    }
}

fn is_slots_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == "$slots",
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some("$slots")
        }
        MemberProp::PrivateName(_) => false,
    }
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

fn is_create_text_vnode_call(args: &[ExprOrSpread]) -> bool {
    matches!(
        args.get(1).map(|arg| arg.expr.as_ref()),
        Some(Expr::Lit(Lit::Num(_)))
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
            if is_setup_props_alias(init, ctx) {
                ctx.setup_props_aliases.insert(binding.id.sym.clone());
                continue;
            }
            let Some(value) = computed_value_expr(init, ctx)? else {
                continue;
            };
            ctx.setup_value_bindings
                .insert(binding.id.sym.clone(), value);
        }
    }

    Ok(())
}

fn is_setup_props_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.setup_props_context
        .as_ref()
        .is_some_and(|setup_props| setup_props == &ident.sym)
        || ctx.setup_props_aliases.contains(&ident.sym)
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

pub(super) fn setup_props_param(render: RenderSource<'_>) -> Option<Atom> {
    match render {
        RenderSource::SetupArrow {
            setup_props: Some(setup_props),
            ..
        } => Some(setup_props.sym.clone()),
        _ => None,
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

fn computed_value_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    if !is_computed_call(call, ctx) {
        return Ok(None);
    }
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    computed_getter_expr(arg.expr.as_ref(), ctx)
}

fn is_computed_call(call: &CallExpr, ctx: &VueRecoveryContext) -> bool {
    if helper_name(&call.callee, ctx) == Some(VueHelper::Computed) {
        return true;
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn computed_getter_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let Expr::Arrow(arrow) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    match arrow.body.as_ref() {
        BlockStmtOrExpr::Expr(expr) => Ok(Some(clean_expr(&print_expr(expr.as_ref(), ctx)?, ctx))),
        BlockStmtOrExpr::BlockStmt(block) => computed_block_value_expr(&block.stmts, ctx),
    }
}

fn computed_block_value_expr(stmts: &[Stmt], ctx: &VueRecoveryContext) -> Result<Option<String>> {
    if let Some(expr) = computed_if_return_chain_expr(stmts, ctx)? {
        return Ok(Some(expr));
    }

    let Some(expr) = stmts.iter().rev().find_map(return_expr_from_stmt) else {
        return Ok(None);
    };
    Ok(Some(clean_expr(&print_expr(expr, ctx)?, ctx)))
}

fn computed_if_return_chain_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<String>> {
    let mut branches = Vec::new();

    for stmt in stmts {
        match stmt {
            Stmt::If(if_stmt) => {
                let Some(expr) = return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                if if_stmt.alt.is_some() {
                    return Ok(None);
                }
                branches.push((
                    clean_expr(&print_expr(if_stmt.test.as_ref(), ctx)?, ctx),
                    clean_expr(&print_expr(expr, ctx)?, ctx),
                ));
            }
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) if !branches.is_empty() => {
                let fallback = clean_expr(&print_expr(expr, ctx)?, ctx);
                return Ok(Some(format_conditional_expr(&branches, fallback)));
            }
            _ => return Ok(None),
        }
    }

    Ok(None)
}

fn return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => block.stmts.iter().find_map(return_expr_from_stmt),
        _ => None,
    }
}

fn format_conditional_expr(branches: &[(String, String)], fallback: String) -> String {
    branches
        .iter()
        .rev()
        .fold(fallback, |alternate, (condition, consequent)| {
            format!("{condition} ? {consequent} : {alternate}")
        })
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
