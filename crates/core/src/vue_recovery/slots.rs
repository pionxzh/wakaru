use anyhow::Result;
use swc_core::ecma::ast::{
    ArrowExpr, Expr, ExprOrSpread, MemberExpr, MemberProp, ObjectLit, ObjectPatProp, Pat, Prop,
    PropOrSpread,
};

use super::attrs::recover_attrs;
use super::expressions::{clean_attr_expr, print_expr, printed_vue_expr};
use super::helpers::{helper_name, VueHelper};
use super::nodes::{
    apply_for_param_renames, arrow_return_expr, clean_condition_expr, is_undefined_expr,
    list_item_context, recover_children, recover_for_params,
};
use super::syntax::{prop_name, string_lit};
use super::VueRecoveryContext;
use crate::vue_template::{
    VueAttr, VueDirective, VueElement, VueExpr, VueFor, VueIfBranch, VueNode,
};

pub(super) fn recover_direct_slot(
    member: &MemberExpr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    if !is_slot_object_expr(member.obj.as_ref(), ctx) {
        return Ok(None);
    }
    let Some(slot_name) = slot_member_name(&member.prop) else {
        return Ok(None);
    };
    let mut attrs = Vec::new();
    if slot_name != "default" {
        attrs.push(VueAttr::Static {
            name: "name".to_string(),
            value: Some(slot_name),
        });
    }
    Ok(Some(VueNode::Element(
        VueElement::new("slot").with_attrs(attrs),
    )))
}

fn is_slot_object_expr(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match expr {
        Expr::Paren(paren) => is_slot_object_expr(paren.expr.as_ref(), ctx),
        Expr::Ident(ident) => {
            matches!(ident.sym.as_ref(), "$slots" | "slots")
                || ctx.slot_bindings.contains(&ident.sym)
        }
        Expr::Member(member) => slot_member_name(&member.prop).as_deref() == Some("$slots"),
        _ => false,
    }
}

fn slot_member_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.to_string()),
        MemberProp::Computed(computed) => string_lit(computed.expr.as_ref()),
        MemberProp::PrivateName(_) => None,
    }
}

pub(super) fn recover_component_children(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<Vec<VueNode>>> {
    match expr {
        Expr::Call(call) if helper_name(&call.callee, ctx) == Some(VueHelper::CreateSlots) => {
            recover_create_slots(&call.args, ctx)
        }
        Expr::Object(object) => recover_component_slots(object, ctx),
        _ => Ok(None),
    }
}

fn recover_create_slots(
    args: &[ExprOrSpread],
    ctx: &VueRecoveryContext,
) -> Result<Option<Vec<VueNode>>> {
    let Some(base_arg) = args.first() else {
        return Ok(None);
    };
    let Expr::Object(base_slots) = base_arg.expr.as_ref() else {
        return Ok(None);
    };
    let Some(mut slots) = recover_component_slots(base_slots, ctx)? else {
        return Ok(None);
    };
    let Some(dynamic_arg) = args.get(1) else {
        return Ok(Some(slots));
    };
    let Expr::Array(dynamic_slots) = dynamic_arg.expr.as_ref() else {
        return Ok(None);
    };

    for elem in dynamic_slots.elems.iter().flatten() {
        let Some(slot) = recover_dynamic_slot(elem.expr.as_ref(), ctx)? else {
            return Ok(None);
        };
        slots.push(slot);
    }

    Ok(Some(slots))
}

fn recover_component_slots(
    object: &ObjectLit,
    ctx: &VueRecoveryContext,
) -> Result<Option<Vec<VueNode>>> {
    let mut slots = Vec::new();
    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return Ok(None);
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return Ok(None);
        };
        let Some(slot_name) = prop_name(&key_value.key) else {
            return Ok(None);
        };
        if slot_name == "_" {
            continue;
        }
        let Some(slot) = recover_component_slot(
            &RecoveredSlotName::Static(slot_name),
            key_value.value.as_ref(),
            ctx,
            Vec::new(),
        )?
        else {
            return Ok(None);
        };
        slots.push(slot);
    }
    Ok(Some(slots))
}

fn recover_dynamic_slot(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    match expr {
        Expr::Paren(paren) => recover_dynamic_slot(paren.expr.as_ref(), ctx),
        Expr::Object(object) => recover_slot_descriptor(object, ctx),
        Expr::Cond(cond) if is_undefined_expr(cond.alt.as_ref()) => {
            let Some(slot) = recover_dynamic_slot(cond.cons.as_ref(), ctx)? else {
                return Ok(None);
            };
            Ok(Some(VueNode::If(vec![VueIfBranch {
                condition: Some(VueExpr::new(clean_condition_expr(cond.test.as_ref(), ctx)?)),
                node: Box::new(slot),
            }])))
        }
        Expr::Call(call) if helper_name(&call.callee, ctx) == Some(VueHelper::RenderList) => {
            recover_dynamic_slot_list(&call.args, ctx)
        }
        _ => Ok(None),
    }
}

fn recover_dynamic_slot_list(
    args: &[ExprOrSpread],
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    let Some(source_arg) = args.first() else {
        return Ok(None);
    };
    let Some(callback_arg) = args.get(1) else {
        return Ok(None);
    };
    let Expr::Arrow(callback) = callback_arg.expr.as_ref() else {
        return Ok(None);
    };
    let Some(for_params) = recover_for_params(&callback.params, "slot", ctx)? else {
        return Ok(None);
    };
    let Some(slot_expr) = arrow_return_expr(&callback.body) else {
        return Ok(None);
    };

    let item_ctx = list_item_context(ctx, &for_params);
    let Some(mut slot) = recover_dynamic_slot(slot_expr, &item_ctx)? else {
        return Ok(None);
    };
    apply_for_param_renames(&mut slot, &for_params);

    Ok(Some(VueNode::For(VueFor {
        value: for_params.value,
        source: VueExpr::new(clean_attr_expr(
            &print_expr(source_arg.expr.as_ref(), ctx)?,
            ctx,
        )),
        node: Box::new(slot),
    })))
}

fn recover_slot_descriptor(
    object: &ObjectLit,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    let mut slot_name = None;
    let mut slot_fn = None;
    let mut extra_attrs = Vec::new();

    for prop in &object.props {
        let PropOrSpread::Prop(prop) = prop else {
            return Ok(None);
        };
        let Prop::KeyValue(key_value) = prop.as_ref() else {
            return Ok(None);
        };
        let Some(name) = prop_name(&key_value.key) else {
            return Ok(None);
        };
        match name.as_str() {
            "name" => {
                slot_name = Some(if let Some(name) = string_lit(key_value.value.as_ref()) {
                    RecoveredSlotName::Static(name)
                } else {
                    RecoveredSlotName::Dynamic(printed_vue_expr(key_value.value.as_ref(), ctx)?)
                });
            }
            "fn" => slot_fn = Some(key_value.value.as_ref()),
            "key" => {
                if string_lit(key_value.value.as_ref()).is_none() {
                    extra_attrs.push(VueAttr::Bind {
                        name: "key".to_string(),
                        expr: printed_vue_expr(key_value.value.as_ref(), ctx)?,
                    });
                };
            }
            _ => return Ok(None),
        }
    }

    let (Some(slot_name), Some(slot_fn)) = (slot_name, slot_fn) else {
        return Ok(None);
    };
    recover_component_slot(&slot_name, slot_fn, ctx, extra_attrs)
}

#[derive(Clone)]
enum RecoveredSlotName {
    Static(String),
    Dynamic(VueExpr),
}

fn recover_component_slot(
    slot_name: &RecoveredSlotName,
    expr: &Expr,
    ctx: &VueRecoveryContext,
    extra_attrs: Vec<VueAttr>,
) -> Result<Option<VueNode>> {
    let Some(slot_fn) = slot_fn_expr(expr, ctx) else {
        return Ok(None);
    };
    let Expr::Arrow(arrow) = slot_fn else {
        return Ok(None);
    };
    let Some(children_expr) = arrow_return_expr(&arrow.body) else {
        return Ok(None);
    };
    let mut directive = match slot_name {
        RecoveredSlotName::Static(name) => VueDirective::new("slot").with_arg(name.clone()),
        RecoveredSlotName::Dynamic(name) => VueDirective::new("slot").with_dynamic_arg(name),
    };
    if let Some(scope) = slot_scope(arrow, ctx)? {
        directive = directive.with_expr(scope);
    }
    let mut attrs = vec![VueAttr::Directive(directive)];
    attrs.extend(extra_attrs);
    Ok(Some(VueNode::Element(
        VueElement::new("template")
            .with_attrs(attrs)
            .with_children(recover_children(children_expr, ctx)?),
    )))
}

fn slot_fn_expr<'a>(expr: &'a Expr, ctx: &VueRecoveryContext) -> Option<&'a Expr> {
    let Expr::Call(call) = expr else {
        return Some(expr);
    };
    if helper_name(&call.callee, ctx) != Some(VueHelper::WithCtx) {
        return Some(expr);
    }
    call.args.first().map(|arg| arg.expr.as_ref())
}

fn slot_scope(arrow: &ArrowExpr, ctx: &VueRecoveryContext) -> Result<Option<VueExpr>> {
    let Some(param) = arrow.params.first() else {
        return Ok(None);
    };
    Ok(slot_pat(param, ctx)?.map(VueExpr::new))
}

pub(super) fn slot_pat(pat: &Pat, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match pat {
        Pat::Ident(binding) => Ok(Some(binding.id.sym.to_string())),
        Pat::Object(object) => {
            let mut props = Vec::new();
            for prop in &object.props {
                let Some(prop) = object_slot_pat_prop(prop, ctx)? else {
                    return Ok(None);
                };
                props.push(prop);
            }
            Ok(Some(format!("{{ {} }}", props.join(", "))))
        }
        _ => Ok(None),
    }
}

fn object_slot_pat_prop(prop: &ObjectPatProp, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match prop {
        ObjectPatProp::Assign(assign) => {
            let name = assign.key.sym.to_string();
            if let Some(value) = &assign.value {
                Ok(Some(format!(
                    "{name} = {}",
                    clean_attr_expr(&print_expr(value.as_ref(), ctx)?, ctx)
                )))
            } else {
                Ok(Some(name))
            }
        }
        ObjectPatProp::KeyValue(key_value) => {
            let Some(name) = prop_name(&key_value.key) else {
                return Ok(None);
            };
            let Some(value) = slot_pat(key_value.value.as_ref(), ctx)? else {
                return Ok(None);
            };
            if value == name {
                Ok(Some(name))
            } else {
                Ok(Some(format!("{name}: {value}")))
            }
        }
        ObjectPatProp::Rest(rest) => match rest.arg.as_ref() {
            Pat::Ident(binding) => Ok(Some(format!("...{}", binding.id.sym))),
            _ => Ok(None),
        },
    }
}

pub(super) fn recover_slot(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let slot_name = args
        .get(1)
        .and_then(|arg| string_lit(arg.expr.as_ref()))
        .unwrap_or_else(|| "default".to_string());
    let mut attrs = Vec::new();
    if slot_name != "default" {
        attrs.push(VueAttr::Static {
            name: "name".to_string(),
            value: Some(slot_name),
        });
    }
    if let Some(props_arg) = args.get(2) {
        attrs.extend(recover_attrs(props_arg.expr.as_ref(), ctx)?);
    }

    let children = args
        .get(3)
        .map(|arg| recover_slot_fallback(arg.expr.as_ref(), ctx))
        .transpose()?
        .unwrap_or_default();

    Ok(VueNode::Element(
        VueElement::new("slot")
            .with_attrs(attrs)
            .with_children(children),
    ))
}

fn recover_slot_fallback(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueNode>> {
    let Expr::Arrow(arrow) = expr else {
        return Ok(Vec::new());
    };
    let Some(fallback) = arrow_return_expr(&arrow.body) else {
        return Ok(Vec::new());
    };
    recover_children(fallback, ctx)
}
