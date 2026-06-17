use swc_core::ecma::ast::{Callee, Expr};

use super::VueRecoveryContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VueHelper {
    CreateBlock,
    CreateElementBlock,
    CreateElementVNode,
    CreateTextVNode,
    CreateVNode,
    Fragment,
    RenderList,
    RenderSlot,
    ResolveComponent,
    ResolveDirective,
    ResolveDynamicComponent,
    ToDisplayString,
    VModel(String),
    VShow,
    WithDirectives,
    WithKeys,
    WithModifiers,
    Other(String),
}

impl VueHelper {
    pub(super) fn from_imported_name(name: String) -> Self {
        match name.as_str() {
            "createBlock" => Self::CreateBlock,
            "createElementBlock" => Self::CreateElementBlock,
            "createElementVNode" => Self::CreateElementVNode,
            "createTextVNode" => Self::CreateTextVNode,
            "createVNode" => Self::CreateVNode,
            "Fragment" => Self::Fragment,
            "renderList" => Self::RenderList,
            "renderSlot" => Self::RenderSlot,
            "resolveComponent" => Self::ResolveComponent,
            "resolveDirective" => Self::ResolveDirective,
            "resolveDynamicComponent" => Self::ResolveDynamicComponent,
            "toDisplayString" => Self::ToDisplayString,
            "vShow" => Self::VShow,
            "withDirectives" => Self::WithDirectives,
            "withKeys" => Self::WithKeys,
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
    match expr.as_ref() {
        Expr::Ident(ident) => ctx.vue_helpers.get(&ident.sym).cloned(),
        _ => None,
    }
}

pub(super) fn is_fragment_tag(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match expr {
        Expr::Ident(ident) => ctx
            .vue_helpers
            .get(&ident.sym)
            .map(|helper| helper == &VueHelper::Fragment)
            .unwrap_or_else(|| ident.sym.as_ref() == "Fragment"),
        _ => false,
    }
}
