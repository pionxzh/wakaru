use swc_core::ecma::ast::{Callee, Expr, MemberExpr, MemberProp};

use super::VueRecoveryContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VueHelper {
    Computed,
    CreateBlock,
    CreateCommentVNode,
    CreateElementBlock,
    CreateElementVNode,
    CreateSlots,
    CreateStaticVNode,
    CreateTextVNode,
    CreateVNode,
    Fragment,
    OpenBlock,
    RenderList,
    RenderSlot,
    ResolveComponent,
    ResolveDirective,
    ResolveDynamicComponent,
    ToDisplayString,
    Unref,
    VModel(String),
    VShow,
    WithCtx,
    WithDirectives,
    WithKeys,
    WithMemo,
    WithModifiers,
    Other(String),
}

impl VueHelper {
    pub(super) fn from_imported_name(name: String) -> Self {
        match name.as_str() {
            "computed" => Self::Computed,
            "createBlock" => Self::CreateBlock,
            "createCommentVNode" => Self::CreateCommentVNode,
            "createElementBlock" => Self::CreateElementBlock,
            "createElementVNode" => Self::CreateElementVNode,
            "createSlots" => Self::CreateSlots,
            "createStaticVNode" => Self::CreateStaticVNode,
            "createTextVNode" => Self::CreateTextVNode,
            "createVNode" => Self::CreateVNode,
            "Fragment" => Self::Fragment,
            "openBlock" => Self::OpenBlock,
            "renderList" => Self::RenderList,
            "renderSlot" => Self::RenderSlot,
            "resolveComponent" => Self::ResolveComponent,
            "resolveDirective" => Self::ResolveDirective,
            "resolveDynamicComponent" => Self::ResolveDynamicComponent,
            "toDisplayString" => Self::ToDisplayString,
            "unref" => Self::Unref,
            "vShow" => Self::VShow,
            "withCtx" => Self::WithCtx,
            "withDirectives" => Self::WithDirectives,
            "withKeys" => Self::WithKeys,
            "withMemo" => Self::WithMemo,
            "withModifiers" => Self::WithModifiers,
            helper if helper.starts_with("vModel") => Self::VModel(name),
            _ => Self::Other(name),
        }
    }
}

pub(super) fn helper_call_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<VueHelper> {
    let Expr::Call(call) = expr else {
        return None;
    };
    helper_name(&call.callee, ctx)
}

pub(super) fn helper_name(callee: &Callee, ctx: &VueRecoveryContext) -> Option<VueHelper> {
    let Callee::Expr(expr) = callee else {
        return None;
    };
    helper_name_from_expr(expr.as_ref(), ctx)
}

pub(super) fn is_fragment_tag(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    if helper_name_from_expr(expr, ctx) == Some(VueHelper::Fragment) {
        return true;
    }

    match expr {
        Expr::Ident(ident) => ctx
            .vue_helpers
            .get(&ident.sym)
            .filter(|_| ctx.resolves_to_import(ident))
            .map(|helper| helper == &VueHelper::Fragment)
            .unwrap_or_else(|| {
                ident.sym.as_ref() == "Fragment" && ident.ctxt == ctx.unresolved_ctxt
            }),
        _ => false,
    }
}

fn helper_name_from_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Option<VueHelper> {
    match expr {
        Expr::Ident(ident) => ctx
            .vue_helpers
            .get(&ident.sym)
            .filter(|_| ctx.resolves_to_import(ident))
            .cloned(),
        Expr::Member(member) => namespace_helper_name(member, ctx),
        _ => None,
    }
}

fn namespace_helper_name(member: &MemberExpr, ctx: &VueRecoveryContext) -> Option<VueHelper> {
    let Expr::Ident(object) = member.obj.as_ref() else {
        return None;
    };
    if !ctx.vue_namespaces.contains(&object.sym) || !ctx.resolves_to_import(object) {
        return None;
    }

    let name = match &member.prop {
        MemberProp::Ident(ident) => ident.sym.to_string(),
        MemberProp::Computed(_) | MemberProp::PrivateName(_) => return None,
    };
    Some(VueHelper::from_imported_name(name))
}
