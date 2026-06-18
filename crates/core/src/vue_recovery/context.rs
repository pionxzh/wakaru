use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, SourceMap, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BindingIdent, BlockStmtOrExpr, CallExpr, Callee,
    ClassDecl, Decl, Expr, ExprOrSpread, FnDecl, Function, Ident, IfStmt, ImportSpecifier, Lit,
    MemberExpr, MemberProp, Module, ModuleDecl, ModuleItem, ObjectLit, ObjectPat, ObjectPatProp,
    Pat, Prop, PropName, PropOrSpread, ReturnStmt, SimpleAssignTarget, Stmt, UpdateExpr, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::expressions::{clean_expr, print_expr};
use super::helpers::{helper_name, VueHelper};
use super::syntax::{
    module_export_name, param_binding_ident, prop_name, string_lit, wtf8_to_string,
};
use super::{RenderSource, VueRecoveryContext, VueScriptImport, VueSetupRefBinding};
use crate::js_names::is_valid_identifier_name;

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
                                if source.contains("vue") {
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
                                    .insert(default.local.sym.clone(), component.clone());
                            }
                        }
                        ImportSpecifier::Namespace(namespace) => {
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
        if let Some(ref_props) = provider_ref_props_from_init(init, ctx) {
            ctx.provider_ref_bindings
                .insert(binding.id.sym.clone(), ref_props);
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

    let mut provider_ref_object_bindings = HashMap::new();

    for stmt in setup_stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match &decl.name {
                Pat::Ident(binding) => {
                    if is_setup_props_alias(init, ctx) {
                        ctx.setup_props_aliases.insert(binding.id.sym.clone());
                        continue;
                    }
                    if let Some(ref_props) =
                        setup_provider_ref_props(init, ctx, &provider_ref_object_bindings)
                    {
                        provider_ref_object_bindings.insert(binding.id.sym.clone(), ref_props);
                    }
                    if is_ref_object_expr(init, ctx) {
                        ctx.setup_ref_object_bindings.insert(binding.id.sym.clone());
                    }
                    if let Some(value) = computed_value_expr(init, ctx)? {
                        ctx.setup_value_bindings
                            .insert(binding.id.sym.clone(), value);
                        continue;
                    }
                    if let Some((value, import_refs)) = computed_script_setup_expr(init, ctx)? {
                        ctx.setup_script_import_refs.extend(import_refs);
                        ctx.setup_script_bindings
                            .push((binding.id.sym.clone(), value));
                        ctx.setup_ref_bindings.insert(binding.id.sym.clone());
                        continue;
                    }
                    if is_ref_like_value_expr(init, ctx) {
                        if let Some((expr, helper, known_ref)) = ref_script_setup_expr(init, ctx)? {
                            ctx.setup_ref_script_bindings.push(VueSetupRefBinding {
                                binding: binding.id.sym.clone(),
                                expr,
                                helper,
                                known_ref,
                            });
                        }
                        ctx.setup_ref_bindings.insert(binding.id.sym.clone());
                    }
                }
                Pat::Object(object)
                    if is_ref_object_expr(init, ctx) || is_ref_object_alias(init, ctx) =>
                {
                    collect_object_pat_bindings(object, &mut ctx.setup_ref_bindings);
                }
                Pat::Object(object) => {
                    if let Some(ref_props) =
                        setup_provider_ref_props(init, ctx, &provider_ref_object_bindings)
                    {
                        collect_provider_object_pat_bindings(
                            object,
                            &ref_props,
                            &mut ctx.setup_ref_bindings,
                        );
                    }
                }
                _ => {}
            }
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

fn is_ref_like_value_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Computed) => return true,
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn is_ref_like_vue_helper(name: &str) -> bool {
    matches!(name, "ref" | "shallowRef" | "customRef" | "toRef")
}

fn ref_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<(String, String, bool)>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    let Some(helper) = ref_script_setup_helper(call, ctx) else {
        return Ok(None);
    };
    let mut args = Vec::new();
    for arg in &call.args {
        let mut printed = clean_expr(&print_expr(arg.expr.as_ref(), ctx)?, ctx);
        if arg.spread.is_some() {
            printed = format!("...{printed}");
        }
        args.push(printed);
    }
    let known_ref = helper_name(&call.callee, ctx).is_some_and(
        |helper| matches!(helper, VueHelper::Other(name) if is_ref_like_vue_helper(&name)),
    );
    Ok(Some((
        format!("{helper}({})", args.join(", ")),
        helper,
        known_ref,
    )))
}

fn ref_script_setup_helper(call: &CallExpr, ctx: &VueRecoveryContext) -> Option<String> {
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Other(name)) if is_ref_like_vue_helper(&name) => Some(name),
        _ => call_callee_ident(call)
            .filter(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
            .map(|_| "ref".to_string()),
    }
}

fn is_ref_object_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return false;
    };
    match helper_name(&call.callee, ctx) {
        Some(VueHelper::Other(name)) if is_ref_object_helper(&name) => return true,
        _ => {}
    }
    call_callee_ident(call).is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn is_ref_object_alias(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ctx.setup_ref_object_bindings.contains(&ident.sym)
}

fn is_ref_object_helper(name: &str) -> bool {
    matches!(name, "toRefs" | "storeToRefs")
}

fn provider_ref_props_from_init(expr: &Expr, ctx: &VueRecoveryContext) -> Option<HashSet<Atom>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };

    call.args
        .iter()
        .filter_map(|arg| provider_ref_props_from_callback(arg.expr.as_ref(), ctx))
        .find(|ref_props| !ref_props.is_empty())
}

fn provider_ref_props_from_callback(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    match unwrap_paren_expr(expr) {
        Expr::Arrow(arrow) => match arrow.body.as_ref() {
            BlockStmtOrExpr::BlockStmt(block) => {
                provider_ref_props_from_stmts(block.stmts.as_slice(), ctx)
            }
            BlockStmtOrExpr::Expr(expr) => provider_ref_props_from_return_expr(expr.as_ref(), ctx),
        },
        Expr::Fn(function) => function
            .function
            .body
            .as_ref()
            .and_then(|body| provider_ref_props_from_stmts(body.stmts.as_slice(), ctx)),
        _ => None,
    }
}

fn provider_ref_props_from_stmts(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let refs = collect_provider_ref_bindings(stmts, ctx);
    let object = stmts.iter().rev().find_map(return_expr_from_stmt)?;
    provider_ref_props_from_return_expr_with_refs(object, &refs, ctx)
}

fn provider_ref_props_from_return_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let refs = HashSet::new();
    provider_ref_props_from_return_expr_with_refs(expr, &refs, ctx)
}

fn provider_ref_props_from_return_expr_with_refs(
    expr: &Expr,
    refs: &HashSet<Atom>,
    ctx: &VueRecoveryContext,
) -> Option<HashSet<Atom>> {
    let Expr::Object(object) = unwrap_paren_expr(expr) else {
        return None;
    };
    let mut ref_props = HashSet::new();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            continue;
        };
        match prop.as_ref() {
            Prop::Shorthand(ident) if refs.contains(&ident.sym) => {
                ref_props.insert(ident.sym.clone());
            }
            Prop::KeyValue(key_value) => {
                let value = unwrap_paren_expr(key_value.value.as_ref());
                let is_ref_value = match value {
                    Expr::Ident(value) => refs.contains(&value.sym),
                    _ => is_ref_like_value_expr(value, ctx),
                };
                if !is_ref_value {
                    continue;
                }
                if let Some(name) = prop_name(&key_value.key) {
                    ref_props.insert(Atom::from(name));
                }
            }
            _ => {}
        }
    }
    (!ref_props.is_empty()).then_some(ref_props)
}

fn collect_provider_ref_bindings(stmts: &[Stmt], ctx: &VueRecoveryContext) -> HashSet<Atom> {
    let mut ref_bindings = HashSet::new();
    let mut ref_object_bindings = HashSet::new();

    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        for decl in &var.decls {
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            match &decl.name {
                Pat::Ident(binding) => {
                    if is_ref_object_expr(init, ctx) {
                        ref_object_bindings.insert(binding.id.sym.clone());
                    }
                    if is_ref_like_value_expr(init, ctx)
                        || ident_expr(unwrap_paren_expr(init))
                            .is_some_and(|ident| ref_bindings.contains(&ident.sym))
                    {
                        ref_bindings.insert(binding.id.sym.clone());
                    }
                }
                Pat::Object(object)
                    if is_ref_object_expr(init, ctx)
                        || is_provider_ref_object_alias(init, &ref_object_bindings) =>
                {
                    collect_object_pat_bindings(object, &mut ref_bindings);
                }
                _ => {}
            }
        }
    }

    ref_bindings
}

fn is_provider_ref_object_alias(expr: &Expr, ref_object_bindings: &HashSet<Atom>) -> bool {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return false;
    };
    ref_object_bindings.contains(&ident.sym)
}

fn setup_provider_ref_props(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    bindings: &HashMap<Atom, HashSet<Atom>>,
) -> Option<HashSet<Atom>> {
    provider_ref_props_from_expr(expr, ctx)
        .cloned()
        .or_else(|| provider_ref_props_from_alias(expr, bindings).cloned())
}

fn provider_ref_props_from_expr<'a>(
    expr: &Expr,
    ctx: &'a VueRecoveryContext,
) -> Option<&'a HashSet<Atom>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = unwrap_paren_expr(callee.as_ref()) else {
        return None;
    };
    if !is_provider_ref_method(&member.prop) {
        return None;
    }
    let Expr::Ident(provider) = unwrap_paren_expr(member.obj.as_ref()) else {
        return None;
    };
    ctx.provider_ref_bindings.get(&provider.sym)
}

fn is_provider_ref_method(prop: &MemberProp) -> bool {
    matches!(prop, MemberProp::Ident(prop) if matches!(prop.sym.as_ref(), "provide" | "inject"))
}

fn provider_ref_props_from_alias<'a>(
    expr: &Expr,
    bindings: &'a HashMap<Atom, HashSet<Atom>>,
) -> Option<&'a HashSet<Atom>> {
    let Expr::Ident(ident) = unwrap_paren_expr(expr) else {
        return None;
    };
    bindings.get(&ident.sym)
}

fn collect_object_pat_bindings(object: &ObjectPat, bindings: &mut HashSet<Atom>) {
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                collect_pat_bindings(key_value.value.as_ref(), bindings);
            }
            ObjectPatProp::Assign(assign) => {
                bindings.insert(assign.key.sym.clone());
            }
            ObjectPatProp::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
        }
    }
}

fn collect_provider_object_pat_bindings(
    object: &ObjectPat,
    ref_props: &HashSet<Atom>,
    bindings: &mut HashSet<Atom>,
) {
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                let Some(name) = prop_name(&key_value.key) else {
                    continue;
                };
                if ref_props.iter().any(|prop| prop.as_ref() == name.as_str()) {
                    collect_pat_bindings(key_value.value.as_ref(), bindings);
                }
            }
            ObjectPatProp::Assign(assign) => {
                if ref_props.contains(&assign.key.sym) {
                    bindings.insert(assign.key.sym.clone());
                }
            }
            ObjectPatProp::Rest(_) => {}
        }
    }
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut HashSet<Atom>) {
    match pat {
        Pat::Ident(binding) => {
            bindings.insert(binding.id.sym.clone());
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings);
            }
        }
        Pat::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
        Pat::Object(object) => collect_object_pat_bindings(object, bindings),
        Pat::Assign(assign) => collect_pat_bindings(assign.left.as_ref(), bindings),
        Pat::Expr(_) | Pat::Invalid(_) => {}
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

fn computed_script_setup_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<(String, HashSet<Atom>)>> {
    let Expr::Call(call) = unwrap_paren_expr(expr) else {
        return Ok(None);
    };
    let Some(arg) = call.args.first() else {
        return Ok(None);
    };
    if !is_computed_script_setup_call(call, arg.expr.as_ref(), ctx) {
        return Ok(None);
    }
    let getter = clean_expr(&print_expr(arg.expr.as_ref(), ctx)?, ctx);
    let import_refs = script_import_refs(arg.expr.as_ref(), &ctx.script_imports);
    Ok(Some((format!("computed({getter})"), import_refs)))
}

fn is_computed_script_setup_call(call: &CallExpr, getter: &Expr, ctx: &VueRecoveryContext) -> bool {
    let is_getter = matches!(unwrap_paren_expr(getter), Expr::Arrow(_) | Expr::Fn(_));
    if !is_getter {
        return false;
    }
    helper_name(&call.callee, ctx) == Some(VueHelper::Computed)
        || call_callee_ident(call)
            .is_some_and(|callee| ctx.vue_helper_candidates.contains(&callee.sym))
}

fn script_import_refs(expr: &Expr, imports: &HashMap<Atom, VueScriptImport>) -> HashSet<Atom> {
    let mut collector = ScriptImportRefCollector {
        imports,
        scopes: vec![HashSet::new()],
        refs: HashSet::new(),
    };
    expr.visit_with(&mut collector);
    collector.refs
}

struct ScriptImportRefCollector<'a> {
    imports: &'a HashMap<Atom, VueScriptImport>,
    scopes: Vec<HashSet<Atom>>,
    refs: HashSet<Atom>,
}

impl ScriptImportRefCollector<'_> {
    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, sym: &Atom) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(sym.clone());
        }
    }

    fn declare_pat(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(binding) => self.declare(&binding.id.sym),
            Pat::Array(array) => {
                for elem in array.elems.iter().flatten() {
                    self.declare_pat(elem);
                }
            }
            Pat::Object(object) => {
                for prop in &object.props {
                    match prop {
                        ObjectPatProp::KeyValue(key_value) => self.declare_pat(&key_value.value),
                        ObjectPatProp::Assign(assign) => self.declare(&assign.key.sym),
                        ObjectPatProp::Rest(rest) => self.declare_pat(&rest.arg),
                    }
                }
            }
            Pat::Rest(rest) => self.declare_pat(&rest.arg),
            Pat::Assign(assign) => self.declare_pat(&assign.left),
            Pat::Expr(_) | Pat::Invalid(_) => {}
        }
    }

    fn is_shadowed(&self, sym: &Atom) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(sym))
    }
}

impl Visit for ScriptImportRefCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self.imports.contains_key(&ident.sym) && !self.is_shadowed(&ident.sym) {
            self.refs.insert(ident.sym.clone());
        }
    }

    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        self.declare(&ident.id.sym);
    }

    fn visit_prop_name(&mut self, prop: &PropName) {
        if let PropName::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_with(self);
        }
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        self.declare_pat(&declarator.name);
        if let Some(init) = &declarator.init {
            init.visit_with(self);
        }
    }

    fn visit_fn_decl(&mut self, function: &FnDecl) {
        self.declare(&function.ident.sym);
        self.visit_function(&function.function);
    }

    fn visit_function(&mut self, function: &Function) {
        self.push_scope();
        for param in &function.params {
            self.declare_pat(&param.pat);
        }
        if let Some(body) = &function.body {
            body.visit_with(self);
        }
        self.pop_scope();
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.push_scope();
        for param in &arrow.params {
            self.declare_pat(param);
        }
        arrow.body.visit_with(self);
        self.pop_scope();
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        self.declare(&class.ident.sym);
        class.class.visit_with(self);
    }
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

    let Some((return_index, expr)) = computed_final_return_expr(stmts) else {
        return Ok(None);
    };
    let prior_stmts = &stmts[..return_index];
    let local_exprs = computed_block_local_exprs(prior_stmts);
    let mutated_locals = computed_mutated_local_bindings(prior_stmts, &local_exprs);
    if computed_local_ref_counts(expr, &mutated_locals)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_block_locals(expr, prior_stmts);
    let local_exprs = computed_block_local_exprs(prior_stmts);
    if computed_local_ref_counts(&expr, &local_exprs)
        .values()
        .any(|count| *count > 0)
    {
        return Ok(None);
    }
    let expr = inline_computed_setup_prop_aliases(&expr, &stmts[..return_index], ctx);
    Ok(Some(clean_expr(&print_expr(&expr, ctx)?, ctx)))
}

fn computed_final_return_expr(stmts: &[Stmt]) -> Option<(usize, &Expr)> {
    stmts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, stmt)| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some((index, expr.as_ref())),
            _ => None,
        })
}

fn inline_computed_block_locals(expr: &Expr, stmts: &[Stmt]) -> Expr {
    let mut locals = computed_block_local_exprs(stmts);
    if locals.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    while !locals.is_empty() {
        let counts = computed_local_ref_counts(&expr, &locals);
        let inline_bindings = locals
            .iter()
            .filter(|(name, expr)| {
                counts.get(*name).copied().unwrap_or_default() == 1
                    && computed_local_ref_counts(expr, &locals)
                        .values()
                        .all(|count| *count == 0)
            })
            .map(|(name, expr)| (name.clone(), expr.clone()))
            .collect::<HashMap<_, _>>();
        if inline_bindings.is_empty() {
            break;
        }
        for name in inline_bindings.keys() {
            locals.remove(name);
        }
        expr.visit_mut_with(&mut ComputedLocalInliner::new(inline_bindings));
    }

    expr
}

fn inline_computed_setup_prop_aliases(
    expr: &Expr,
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Expr {
    let aliases = computed_setup_prop_alias_exprs(stmts, ctx);
    if aliases.is_empty() {
        return expr.clone();
    }

    inline_computed_alias_expr(expr, &aliases)
}

fn computed_setup_prop_alias_exprs(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> HashMap<Atom, Expr> {
    let mut aliases = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Object(object) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            if !is_setup_props_alias(init, ctx) {
                continue;
            }
            collect_computed_setup_prop_aliases(object, &mut aliases);
        }
    }
    aliases
}

fn collect_computed_setup_prop_alias_var(
    var: &VarDecl,
    ctx: &VueRecoveryContext,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    if var.kind != VarDeclKind::Const || var.decls.is_empty() {
        return false;
    }

    let mut next_aliases = HashMap::new();
    for decl in &var.decls {
        let Pat::Object(object) = &decl.name else {
            return false;
        };
        let Some(init) = decl.init.as_deref() else {
            return false;
        };
        if !is_setup_props_alias(init, ctx) {
            return false;
        }
        if !collect_computed_setup_prop_aliases(object, &mut next_aliases) {
            return false;
        }
    }

    aliases.extend(next_aliases);
    true
}

fn collect_computed_setup_prop_aliases(
    object: &ObjectPat,
    aliases: &mut HashMap<Atom, Expr>,
) -> bool {
    let mut next_aliases = HashMap::new();
    for prop in &object.props {
        match prop {
            ObjectPatProp::KeyValue(key_value) => {
                let Some(name) =
                    prop_name(&key_value.key).filter(|name| is_valid_identifier_name(name))
                else {
                    return false;
                };
                let Some(binding) = ident_binding_from_pat(key_value.value.as_ref()) else {
                    return false;
                };
                next_aliases.insert(
                    binding.sym.clone(),
                    Expr::Ident(Ident::new(name.into(), DUMMY_SP, Default::default())),
                );
            }
            ObjectPatProp::Assign(assign) => {
                let name = assign.key.sym.as_ref();
                if !is_valid_identifier_name(name) {
                    return false;
                }
                next_aliases.insert(
                    assign.key.sym.clone(),
                    Expr::Ident(Ident::new(
                        assign.key.sym.clone(),
                        DUMMY_SP,
                        Default::default(),
                    )),
                );
            }
            ObjectPatProp::Rest(_) => return false,
        }
    }
    if next_aliases.is_empty() {
        return false;
    }

    aliases.extend(next_aliases);
    true
}

fn ident_binding_from_pat(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(binding) => Some(&binding.id),
        Pat::Assign(assign) => ident_binding_from_pat(assign.left.as_ref()),
        _ => None,
    }
}

fn computed_block_local_exprs(stmts: &[Stmt]) -> HashMap<Atom, Expr> {
    let mut locals = HashMap::new();
    for stmt in stmts {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.kind != VarDeclKind::Const {
            continue;
        }
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            locals.insert(binding.id.sym.clone(), init.clone());
        }
    }
    locals
}

fn computed_mutated_local_bindings(
    stmts: &[Stmt],
    locals: &HashMap<Atom, Expr>,
) -> HashMap<Atom, Expr> {
    if locals.is_empty() {
        return HashMap::new();
    }

    let mut detector = ComputedLocalMutationDetector::new(locals.keys().cloned().collect());
    for stmt in stmts {
        stmt.visit_with(&mut detector);
    }
    let mutated = detector.finish();
    locals
        .iter()
        .filter_map(|(name, expr)| {
            mutated
                .contains(name)
                .then_some((name.clone(), expr.clone()))
        })
        .collect()
}

fn computed_local_ref_counts(expr: &Expr, locals: &HashMap<Atom, Expr>) -> HashMap<Atom, usize> {
    let mut counter = ComputedLocalRefCounter::new(locals.keys().cloned().collect());
    expr.visit_with(&mut counter);
    counter.finish()
}

struct ComputedLocalMutationDetector {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    mutated: HashSet<Atom>,
}

impl ComputedLocalMutationDetector {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            mutated: HashSet::new(),
        }
    }

    fn finish(self) -> HashSet<Atom> {
        self.mutated
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn mark_name(&mut self, name: &str) {
        if let Some(index) = self.active_index(name) {
            self.mutated.insert(self.bindings[index].clone());
        }
    }

    fn mark_member_object(&mut self, member: &MemberExpr) {
        if let Expr::Ident(object) = member.obj.as_ref() {
            self.mark_name(object.sym.as_ref());
        }
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalMutationDetector {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.mark_name(binding.id.sym.as_ref());
            }
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                self.mark_member_object(member);
            }
            _ => {}
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        match update.arg.as_ref() {
            Expr::Ident(ident) => self.mark_name(ident.sym.as_ref()),
            Expr::Member(member) => self.mark_member_object(member),
            _ => {}
        }
        update.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = callee.as_ref() {
                self.mark_member_object(member);
            }
        }
        call.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalRefCounter {
    bindings: Vec<Atom>,
    shadow_depths: Vec<usize>,
    counts: Vec<usize>,
}

impl ComputedLocalRefCounter {
    fn new(mut bindings: Vec<Atom>) -> Self {
        bindings.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        let counts = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            counts,
        }
    }

    fn finish(self) -> HashMap<Atom, usize> {
        self.bindings.into_iter().zip(self.counts).collect()
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|(binding, shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl Visit for ComputedLocalRefCounter {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                self.counts[index] += 1;
                return;
            }
        }
        expr.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_ref() {
            body.visit_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

struct ComputedLocalInliner {
    bindings: Vec<(Atom, Expr)>,
    shadow_depths: Vec<usize>,
}

impl ComputedLocalInliner {
    fn new(mut bindings: HashMap<Atom, Expr>) -> Self {
        let mut bindings = bindings.drain().collect::<Vec<_>>();
        bindings.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
        }
    }

    fn active_index(&self, name: &str) -> Option<usize> {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .position(|((binding, _), shadow_depth)| binding.as_ref() == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        let mut param_bindings = HashSet::new();
        for param in params {
            collect_pat_bindings(param, &mut param_bindings);
        }
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, (binding, _))| param_bindings.contains(binding).then_some(index))
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl VisitMut for ComputedLocalInliner {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(ident) = expr {
            if let Some(index) = self.active_index(ident.sym.as_ref()) {
                *expr = self.bindings[index].1.clone();
                expr.visit_mut_children_with(self);
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut swc_core::ecma::ast::ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.body.visit_mut_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut swc_core::ecma::ast::Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        if let Some(body) = function.body.as_mut() {
            body.visit_mut_with(self);
        }
        self.exit_shadowed(&shadowed);
    }
}

fn computed_if_return_chain_expr(
    stmts: &[Stmt],
    ctx: &VueRecoveryContext,
) -> Result<Option<String>> {
    let mut branches = Vec::new();
    let mut aliases = HashMap::new();

    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var))
                if branches.is_empty()
                    && collect_computed_setup_prop_alias_var(var, ctx, &mut aliases) =>
            {
                continue;
            }
            Stmt::If(if_stmt) => {
                let Some(expr) = return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                if if_stmt.alt.is_some() {
                    return Ok(None);
                }
                let test = inline_computed_alias_expr(if_stmt.test.as_ref(), &aliases);
                let expr = inline_computed_alias_expr(expr, &aliases);
                branches.push((
                    clean_expr(&print_expr(&test, ctx)?, ctx),
                    clean_expr(&print_expr(&expr, ctx)?, ctx),
                ));
            }
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) if !branches.is_empty() => {
                let expr = inline_computed_alias_expr(expr, &aliases);
                let fallback = clean_expr(&print_expr(&expr, ctx)?, ctx);
                return Ok(Some(format_conditional_expr(&branches, fallback)));
            }
            _ => return Ok(None),
        }
    }

    Ok(None)
}

fn inline_computed_alias_expr(expr: &Expr, aliases: &HashMap<Atom, Expr>) -> Expr {
    if aliases.is_empty() {
        return expr.clone();
    }

    let mut expr = expr.clone();
    expr.visit_mut_with(&mut ComputedLocalInliner::new(aliases.clone()));
    expr
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
