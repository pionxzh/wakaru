use std::collections::{HashMap, HashSet};

use anyhow::Result;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, BinaryOp, BlockStmtOrExpr, Expr, Function, Lit, ObjectLit, Pat, Prop,
    PropOrSpread, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::directives::directive_modifiers;
use super::expressions::{
    clean_attr_expr, clean_expr, clean_vue_expr, print_expr, printed_vue_expr,
};
use super::helpers::{helper_call_name, helper_name, VueHelper};
use super::syntax::{prop_name, string_lit, wtf8_to_string};
use super::VueRecoveryContext;
use crate::vue_template::{VueAttr, VueDirective, VueDirectiveArg, VueExpr};

#[derive(Clone, Copy)]
enum AttrOwner {
    Native,
    Component,
}

pub(super) fn recover_attrs(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueAttr>> {
    recover_attrs_for_owner(expr, ctx, AttrOwner::Native)
}

fn recover_attrs_for_owner(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    owner: AttrOwner,
) -> Result<Vec<VueAttr>> {
    match expr {
        Expr::Lit(Lit::Null(_)) => Ok(Vec::new()),
        Expr::Object(object) => recover_attrs_from_object(object, ctx, owner),
        Expr::Ident(ident) => {
            if let Some(object) = ctx.object_bindings.get(&ident.sym) {
                recover_attrs_from_object(object, ctx, owner)
            } else {
                Ok(vec![VueAttr::Spread(clean_vue_expr(
                    &print_expr(expr, ctx)?,
                    ctx,
                ))])
            }
        }
        _ => Ok(vec![VueAttr::Spread(clean_vue_expr(
            &print_expr(expr, ctx)?,
            ctx,
        ))]),
    }
}

pub(super) fn recover_component_attrs(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Vec<VueAttr>> {
    let model_modifiers = match expr {
        Expr::Object(object) => component_model_modifiers(object),
        _ => HashMap::new(),
    };
    Ok(collapse_component_model_attrs(
        recover_attrs_for_owner(expr, ctx, AttrOwner::Component)?,
        &model_modifiers,
    ))
}

fn component_model_modifiers(object: &ObjectLit) -> HashMap<String, Vec<String>> {
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
            let model_prop = component_model_prop_from_modifier_attr(&name)?;
            let modifiers = directive_modifiers(key_value.value.as_ref());
            (!modifiers.is_empty()).then_some((model_prop, modifiers))
        })
        .collect()
}

fn component_model_prop_from_modifier_attr(name: &str) -> Option<String> {
    if name == "modelModifiers" {
        return Some("modelValue".to_string());
    }
    name.strip_suffix("Modifiers")
        .filter(|model_prop| !model_prop.is_empty())
        .map(|model_prop| model_prop.to_string())
}

fn collapse_component_model_attrs(
    attrs: Vec<VueAttr>,
    model_modifiers: &HashMap<String, Vec<String>>,
) -> Vec<VueAttr> {
    let bound_props = attrs
        .iter()
        .filter_map(|attr| match attr {
            VueAttr::Bind { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let update_events = attrs
        .iter()
        .filter_map(|attr| match attr {
            VueAttr::On { name, .. } => name
                .strip_prefix("update:")
                .map(|prop_name| prop_name.to_string()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let model_props = bound_props
        .intersection(&update_events)
        .cloned()
        .collect::<HashSet<_>>();
    if model_props.is_empty() {
        return attrs;
    }

    let mut collapsed = Vec::new();
    for attr in attrs {
        match attr {
            VueAttr::Bind { name, expr } if model_props.contains(&name) => {
                let modifiers = model_modifiers.get(&name).cloned().unwrap_or_default();
                collapsed.push(VueAttr::Directive(VueDirective {
                    name: "model".to_string(),
                    arg: (name != "modelValue").then_some(VueDirectiveArg::Static(name)),
                    expr: Some(expr),
                    modifiers,
                    scope: Default::default(),
                }));
            }
            VueAttr::On { name, .. }
                if name
                    .strip_prefix("update:")
                    .is_some_and(|prop_name| model_props.contains(prop_name)) => {}
            VueAttr::Bind { name, .. } | VueAttr::Static { name, .. }
                if component_model_prop_from_modifier_attr(&name)
                    .is_some_and(|model_prop| model_props.contains(&model_prop)) => {}
            _ => collapsed.push(attr),
        }
    }

    collapsed
}

fn recover_attrs_from_object(
    object: &ObjectLit,
    ctx: &VueRecoveryContext,
    owner: AttrOwner,
) -> Result<Vec<VueAttr>> {
    let mut attrs = Vec::new();
    for prop in &object.props {
        match prop {
            PropOrSpread::Spread(spread) => {
                attrs.push(VueAttr::Spread(clean_vue_expr(
                    &print_expr(spread.expr.as_ref(), ctx)?,
                    ctx,
                )));
            }
            PropOrSpread::Prop(prop) => match prop.as_ref() {
                Prop::KeyValue(key_value) => {
                    let Some(name) = prop_name(&key_value.key) else {
                        attrs.push(VueAttr::Spread(clean_vue_expr(
                            &print_expr(
                                &Expr::Object(ObjectLit {
                                    span: DUMMY_SP,
                                    props: vec![PropOrSpread::Prop(prop.clone())],
                                }),
                                ctx,
                            )?,
                            ctx,
                        )));
                        continue;
                    };
                    attrs.extend(attrs_from_key_value(
                        &name,
                        key_value.value.as_ref(),
                        ctx,
                        owner,
                    )?);
                }
                Prop::Shorthand(ident) => {
                    let name = ident.sym.to_string();
                    attrs.push(shorthand_attr(&name, owner));
                }
                _ => {}
            },
        }
    }
    Ok(collapse_template_ref_attrs(attrs))
}

fn shorthand_attr(name: &str, owner: AttrOwner) -> VueAttr {
    if let Some(event_name) = event_name_from_prop(name, owner) {
        return VueAttr::On {
            name: event_name,
            expr: VueExpr::new(name.to_string()),
            modifiers: Vec::new(),
        };
    }

    VueAttr::Bind {
        name: name.to_string(),
        expr: VueExpr::new(name.to_string()),
    }
}

fn collapse_template_ref_attrs(attrs: Vec<VueAttr>) -> Vec<VueAttr> {
    let ref_key = attrs.iter().find_map(|attr| match attr {
        VueAttr::Static {
            name,
            value: Some(value),
        } if name == "ref_key" => Some(value.clone()),
        _ => None,
    });
    let has_ref = attrs
        .iter()
        .any(|attr| matches!(attr, VueAttr::Bind { name, .. } | VueAttr::Static { name, .. } if name == "ref"));

    let mut collapsed = Vec::new();
    let mut emitted_ref = false;
    for attr in attrs {
        match attr {
            VueAttr::Static { name, .. } if name == "ref_for" => {}
            VueAttr::Static { name, .. } if name == "ref_key" && ref_key.is_some() && has_ref => {
                if !emitted_ref {
                    collapsed.push(VueAttr::Static {
                        name: "ref".to_string(),
                        value: ref_key.clone(),
                    });
                    emitted_ref = true;
                }
            }
            VueAttr::Bind { name, .. } | VueAttr::Static { name, .. }
                if name == "ref" && ref_key.is_some() => {}
            _ => collapsed.push(attr),
        }
    }
    collapsed
}

fn attrs_from_key_value(
    name: &str,
    value: &Expr,
    ctx: &VueRecoveryContext,
    owner: AttrOwner,
) -> Result<Vec<VueAttr>> {
    if let Some(event_name) = event_name_from_prop(name, owner) {
        let event = recover_event_expr(value, ctx)?;
        return Ok(vec![VueAttr::On {
            name: event_name,
            expr: event.expr,
            modifiers: event.modifiers,
        }]);
    }

    if let Some(directive_name) = html_directive_name(name) {
        return Ok(vec![VueAttr::Directive(VueDirective {
            name: directive_name.to_string(),
            arg: None,
            expr: Some(printed_vue_expr(value, ctx)?),
            modifiers: Vec::new(),
            scope: Default::default(),
        })]);
    }

    if name == "class" {
        if let Some(attrs) = class_attrs_from_helper(value, ctx)? {
            return Ok(attrs);
        }
    }

    if name == "style" && helper_call_name(value, ctx).is_some() {
        return Ok(vec![VueAttr::Bind {
            name: name.to_string(),
            expr: VueExpr::new(helper_first_arg_expr(value, ctx)?),
        }]);
    }

    match value {
        Expr::Lit(Lit::Str(str)) => Ok(vec![VueAttr::Static {
            name: name.to_string(),
            value: Some(wtf8_to_string(&str.value)),
        }]),
        Expr::Lit(Lit::Bool(bool)) if bool.value => Ok(vec![VueAttr::Static {
            name: name.to_string(),
            value: None,
        }]),
        _ => Ok(vec![VueAttr::Bind {
            name: name.to_string(),
            expr: printed_vue_expr(value, ctx)?,
        }]),
    }
}

fn html_directive_name(name: &str) -> Option<&'static str> {
    match name {
        "innerHTML" => Some("html"),
        "textContent" => Some("text"),
        _ => None,
    }
}

fn event_name_from_prop(name: &str, owner: AttrOwner) -> Option<String> {
    let event_name = name
        .strip_prefix("on")
        .filter(|s| !s.is_empty())
        .map(lower_first)?;
    Some(match owner {
        AttrOwner::Native => event_name,
        AttrOwner::Component => normalize_component_event_name(&event_name),
    })
}

fn normalize_component_event_name(event_name: &str) -> String {
    if event_name.starts_with("update:") || is_on_prefixed_event_name(event_name) {
        return event_name.to_string();
    }
    kebab_case(event_name)
}

fn is_on_prefixed_event_name(event_name: &str) -> bool {
    let Some(rest) = event_name.strip_prefix("on") else {
        return false;
    };
    rest.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn kebab_case(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn class_attrs_from_helper(value: &Expr, ctx: &VueRecoveryContext) -> Result<Option<Vec<VueAttr>>> {
    if helper_call_name(value, ctx).is_none() {
        return Ok(None);
    }
    let Expr::Call(call) = value else {
        return Ok(None);
    };
    let Some(first) = call.args.first() else {
        return Ok(None);
    };

    let Expr::Array(array) = first.expr.as_ref() else {
        return Ok(Some(vec![VueAttr::Bind {
            name: "class".to_string(),
            expr: VueExpr::new(helper_first_arg_expr(value, ctx)?),
        }]));
    };

    let mut static_classes = Vec::new();
    let mut attrs = Vec::new();
    for elem in array.elems.iter().flatten() {
        if let Expr::Lit(Lit::Str(str)) = elem.expr.as_ref() {
            static_classes.push(wtf8_to_string(&str.value));
        } else {
            attrs.push(VueAttr::Bind {
                name: "class".to_string(),
                expr: printed_vue_expr(elem.expr.as_ref(), ctx)?,
            });
        }
    }

    if !static_classes.is_empty() {
        attrs.insert(
            0,
            VueAttr::Static {
                name: "class".to_string(),
                value: Some(static_classes.join(" ")),
            },
        );
    }
    Ok(Some(attrs))
}

struct RecoveredEventExpr {
    expr: VueExpr,
    modifiers: Vec<String>,
}

fn recover_event_expr(value: &Expr, ctx: &VueRecoveryContext) -> Result<RecoveredEventExpr> {
    if let Some(handler) = cached_event_modifier_handler(value, ctx) {
        return recover_event_expr(handler, ctx);
    }

    if let Expr::Call(call) = value {
        if matches!(
            helper_name(&call.callee, ctx),
            Some(VueHelper::WithModifiers | VueHelper::WithKeys)
        ) {
            if let Some(handler) = call.args.first() {
                let mut event = recover_event_expr(handler.expr.as_ref(), ctx)?;
                if let Some(modifiers) = call.args.get(1) {
                    let mut current = event_modifier_names(modifiers.expr.as_ref());
                    current.append(&mut event.modifiers);
                    event.modifiers = current;
                }
                if !event.modifiers.is_empty() && is_noop_event_handler(handler.expr.as_ref()) {
                    event.expr = VueExpr::new("");
                }
                return Ok(event);
            }
        }
    }

    if let Some(handler) = cached_event_handler_name(value, ctx)? {
        return Ok(RecoveredEventExpr {
            expr: VueExpr::new(handler),
            modifiers: Vec::new(),
        });
    }
    Ok(RecoveredEventExpr {
        expr: printed_vue_expr(value, ctx)?,
        modifiers: Vec::new(),
    })
}

fn cached_event_modifier_handler<'a>(
    value: &'a Expr,
    ctx: &VueRecoveryContext,
) -> Option<&'a Expr> {
    let handler = cached_event_assignment_rhs(value)?;
    match handler {
        Expr::Call(call)
            if matches!(
                helper_name(&call.callee, ctx),
                Some(VueHelper::WithModifiers | VueHelper::WithKeys)
            ) =>
        {
            Some(handler)
        }
        _ => None,
    }
}

fn cached_event_assignment_rhs(value: &Expr) -> Option<&Expr> {
    match value {
        Expr::Paren(paren) => cached_event_assignment_rhs(paren.expr.as_ref()),
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            cached_event_assignment_rhs(bin.right.as_ref())
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => Some(assign.right.as_ref()),
        _ => None,
    }
}

fn is_noop_event_handler(value: &Expr) -> bool {
    match value {
        Expr::Paren(paren) => is_noop_event_handler(paren.expr.as_ref()),
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            is_noop_event_handler(bin.right.as_ref())
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            is_noop_event_handler(assign.right.as_ref())
        }
        Expr::Arrow(arrow) => {
            matches!(arrow.body.as_ref(), BlockStmtOrExpr::BlockStmt(block) if block.stmts.is_empty())
        }
        Expr::Fn(function) => function
            .function
            .body
            .as_ref()
            .is_some_and(|body| body.stmts.is_empty()),
        _ => false,
    }
}

fn event_modifier_names(expr: &Expr) -> Vec<String> {
    let Expr::Array(array) = expr else {
        return Vec::new();
    };
    array
        .elems
        .iter()
        .flatten()
        .filter_map(|elem| string_lit(elem.expr.as_ref()))
        .collect()
}

fn cached_event_handler_name(value: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match value {
        Expr::Paren(paren) => cached_event_handler_name(paren.expr.as_ref(), ctx),
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => {
            cached_event_handler_name(bin.right.as_ref(), ctx)
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            cached_handler_name(assign.right.as_ref(), ctx)
        }
        Expr::Arrow(arrow) => arrow_handler_expr(arrow, ctx),
        Expr::Fn(function) => function_handler_expr(&function.function, ctx),
        _ => Ok(None),
    }
}

fn cached_handler_name(body: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match body {
        Expr::Arrow(arrow) => arrow_handler_expr(arrow, ctx),
        Expr::Fn(function) => function_handler_expr(&function.function, ctx),
        _ => Ok(None),
    }
}

fn arrow_handler_expr(arrow: &ArrowExpr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let BlockStmtOrExpr::Expr(expr) = arrow.body.as_ref() else {
        return Ok(None);
    };
    handler_expr_name(expr.as_ref(), ctx, arrow_event_param(arrow))
}

fn function_handler_expr(function: &Function, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    let Some(body) = function.body.as_ref() else {
        return Ok(None);
    };
    let [Stmt::Return(return_stmt)] = body.stmts.as_slice() else {
        return Ok(None);
    };
    let Some(expr) = return_stmt.arg.as_deref() else {
        return Ok(None);
    };
    handler_expr_name(expr, ctx, function_event_param(function))
}

fn handler_expr_name(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    event_param: Option<&str>,
) -> Result<Option<String>> {
    match expr {
        Expr::Paren(paren) => handler_expr_name(paren.expr.as_ref(), ctx, event_param),
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            clean_event_handler_expr(bin.left.as_ref(), ctx, event_param).map(Some)
        }
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            clean_event_handler_expr(expr, ctx, event_param).map(Some)
        }
        Expr::Call(_) => clean_event_handler_expr(expr, ctx, event_param).map(Some),
        _ => Ok(None),
    }
}

fn clean_event_handler_expr(
    expr: &Expr,
    ctx: &VueRecoveryContext,
    event_param: Option<&str>,
) -> Result<String> {
    if let Some(param) = event_param {
        let mut expr = expr.clone();
        expr.visit_mut_with(&mut EventParamRenamer::new(param));
        return Ok(clean_attr_expr(&print_expr(&expr, ctx)?, ctx));
    }
    Ok(clean_attr_expr(&print_expr(expr, ctx)?, ctx))
}

fn arrow_event_param(arrow: &ArrowExpr) -> Option<&str> {
    let Pat::Ident(binding) = arrow.params.first()? else {
        return None;
    };
    Some(binding.id.sym.as_ref())
}

fn function_event_param(function: &Function) -> Option<&str> {
    let Pat::Ident(binding) = &function.params.first()?.pat else {
        return None;
    };
    Some(binding.id.sym.as_ref())
}

struct EventParamRenamer<'a> {
    param: &'a str,
    shadow_depth: usize,
}

impl<'a> EventParamRenamer<'a> {
    fn new(param: &'a str) -> Self {
        Self {
            param,
            shadow_depth: 0,
        }
    }

    fn is_shadowing_pat(&self, pat: &Pat) -> bool {
        matches!(pat, Pat::Ident(binding) if binding.id.sym.as_ref() == self.param)
    }
}

impl VisitMut for EventParamRenamer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if self.shadow_depth == 0 {
            if let Expr::Ident(ident) = expr {
                if ident.sym.as_ref() == self.param {
                    ident.sym = "$event".into();
                    return;
                }
            }
        }

        expr.visit_mut_children_with(self);
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let shadows = arrow.params.iter().any(|pat| self.is_shadowing_pat(pat));
        if shadows {
            self.shadow_depth += 1;
        }
        arrow.visit_mut_children_with(self);
        if shadows {
            self.shadow_depth -= 1;
        }
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let shadows = function
            .params
            .iter()
            .any(|param| self.is_shadowing_pat(&param.pat));
        if shadows {
            self.shadow_depth += 1;
        }
        function.visit_mut_children_with(self);
        if shadows {
            self.shadow_depth -= 1;
        }
    }
}

fn helper_first_arg_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let Expr::Call(call) = expr else {
        return Ok(clean_expr(&print_expr(expr, ctx)?, ctx));
    };
    let Some(first) = call.args.first() else {
        return Ok(clean_expr(&print_expr(expr, ctx)?, ctx));
    };
    Ok(clean_attr_expr(&print_expr(first.expr.as_ref(), ctx)?, ctx))
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_ascii_lowercase().to_string() + chars.as_str()
}
