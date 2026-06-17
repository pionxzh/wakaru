use anyhow::Result;
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, BinaryOp, BlockStmtOrExpr, Expr, ExprOrSpread, FnDecl, Lit, ObjectLit,
    ObjectPatProp, Pat, Prop, PropOrSpread, ReturnStmt, Stmt,
};

use super::attrs::{recover_attrs, recover_component_attrs};
use super::directives::recover_directive_tuple;
use super::expressions::{clean_attr_expr, clean_expr, print_expr, printed_vue_expr, raw_expr};
use super::helpers::{helper_name, is_fragment_tag, VueHelper};
use super::syntax::{pat_binding_ident, prop_name, string_lit, wtf8_to_string};
use super::VueRecoveryContext;
use crate::vue_template::{
    VueAttr, VueDirective, VueDirectiveArg, VueElement, VueExpr, VueFor, VueIfBranch, VueNode,
};

pub(super) fn recover_render_root(
    render: &FnDecl,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    if let Some(node) = recover_render_if_chain(render, ctx)? {
        return Ok(Some(node));
    }
    let Some(root_expr) = find_render_return(render) else {
        return Ok(None);
    };
    recover_node(root_expr, ctx)
}

fn recover_render_if_chain(render: &FnDecl, ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    let Some(body) = render.function.body.as_ref() else {
        return Ok(None);
    };
    let mut branches = Vec::new();
    let mut in_chain = false;

    for stmt in &body.stmts {
        match stmt {
            Stmt::If(if_stmt) => {
                let Some(expr) = return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                let Some(node) = recover_node(expr, ctx)? else {
                    continue;
                };
                branches.push(VueIfBranch {
                    condition: Some(VueExpr::new(clean_condition_expr(
                        if_stmt.test.as_ref(),
                        ctx,
                    )?)),
                    node: Box::new(node),
                });
                in_chain = true;
            }
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) if in_chain => {
                if let Some(node) = recover_node(expr.as_ref(), ctx)? {
                    branches.push(VueIfBranch {
                        condition: None,
                        node: Box::new(node),
                    });
                }
                return Ok(Some(VueNode::If(branches)));
            }
            _ if in_chain => return Ok(None),
            _ => {}
        }
    }

    Ok(None)
}

fn return_expr_from_stmt(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        Stmt::Block(block) => block.stmts.iter().find_map(|stmt| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some(expr.as_ref()),
            _ => None,
        }),
        _ => None,
    }
}

fn find_render_return(render: &FnDecl) -> Option<&Expr> {
    let body = render.function.body.as_ref()?;
    body.stmts.iter().rev().find_map(|stmt| match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        _ => None,
    })
}

fn recover_node(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    match expr {
        Expr::Paren(paren) => recover_node(paren.expr.as_ref(), ctx),
        Expr::Seq(seq) => {
            let Some(last) = seq.exprs.last() else {
                return Ok(None);
            };
            recover_node(last.as_ref(), ctx)
        }
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalOr => recover_node(bin.right.as_ref(), ctx),
        Expr::Assign(assign) if assign.op == AssignOp::Assign => {
            recover_node(assign.right.as_ref(), ctx)
        }
        Expr::Cond(cond) => recover_conditional_chain(
            cond.test.as_ref(),
            cond.cons.as_ref(),
            cond.alt.as_ref(),
            ctx,
        )
        .map(Some),
        Expr::Call(call) => {
            let Some(helper) = helper_name(&call.callee, ctx) else {
                return Ok(Some(raw_expr(clean_expr(&print_expr(expr, ctx)?, ctx))));
            };
            match helper {
                VueHelper::CreateElementBlock | VueHelper::CreateElementVNode => {
                    recover_element(&call.args, ctx)
                }
                VueHelper::CreateBlock | VueHelper::CreateVNode => {
                    recover_component_vnode(&call.args, ctx).map(Some)
                }
                VueHelper::CreateCommentVNode => Ok(recover_comment_vnode(&call.args)),
                VueHelper::CreateTextVNode => recover_text_vnode(&call.args, ctx).map(Some),
                VueHelper::RenderSlot => recover_slot(&call.args, ctx).map(Some),
                VueHelper::RenderList => recover_render_list(&call.args, ctx).map(Some),
                VueHelper::WithDirectives => recover_with_directives(&call.args, ctx).map(Some),
                VueHelper::WithMemo => recover_with_memo(&call.args, ctx).map(Some),
                VueHelper::ToDisplayString => {
                    let Some(arg) = call.args.first() else {
                        return Ok(None);
                    };
                    Ok(Some(VueNode::Interpolation(VueExpr::new(clean_expr(
                        &print_expr(arg.expr.as_ref(), ctx)?,
                        ctx,
                    )))))
                }
                _ => Ok(Some(raw_expr(clean_expr(&print_expr(expr, ctx)?, ctx)))),
            }
        }
        Expr::Lit(Lit::Str(str)) => Ok(Some(VueNode::Text(wtf8_to_string(&str.value)))),
        _ => Ok(Some(raw_expr(clean_expr(&print_expr(expr, ctx)?, ctx)))),
    }
}

fn recover_conditional_chain(
    test: &Expr,
    cons: &Expr,
    alt: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<VueNode> {
    let mut branches = Vec::new();
    if let Some(node) = recover_node(cons, ctx)? {
        branches.push(VueIfBranch {
            condition: Some(VueExpr::new(clean_condition_expr(test, ctx)?)),
            node: Box::new(node),
        });
    }

    match alt {
        Expr::Cond(cond) => {
            let node = recover_conditional_chain(
                cond.test.as_ref(),
                cond.cons.as_ref(),
                cond.alt.as_ref(),
                ctx,
            )?;
            match node {
                VueNode::If(mut nested_branches) => branches.append(&mut nested_branches),
                node => branches.push(VueIfBranch {
                    condition: None,
                    node: Box::new(node),
                }),
            }
        }
        _ => {
            if let Some(node) = recover_node(alt, ctx)? {
                branches.push(VueIfBranch {
                    condition: None,
                    node: Box::new(node),
                });
            }
        }
    }

    Ok(VueNode::If(branches))
}

fn recover_render_list(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(source_arg) = args.first() else {
        return Ok(VueNode::RawExpr("renderList()".into()));
    };
    let Some(callback_arg) = args.get(1) else {
        return Ok(raw_expr(clean_expr(
            &print_expr(source_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Expr::Arrow(callback) = callback_arg.expr.as_ref() else {
        return Ok(raw_expr(clean_expr(
            &print_expr(callback_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Some(item_param) = callback
        .params
        .first()
        .and_then(pat_binding_ident)
        .map(|ident| ident.sym.clone())
    else {
        return Ok(raw_expr(clean_expr(
            &print_expr(callback_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Some(item_expr) = arrow_return_expr(&callback.body) else {
        return Ok(raw_expr(clean_expr(
            &print_expr(callback_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };

    let source = VueExpr::new(clean_attr_expr(
        &print_expr(source_arg.expr.as_ref(), ctx)?,
        ctx,
    ));
    let mut item_ctx = ctx.clone();
    item_ctx.render_context = None;
    let Some(mut item_node) = recover_node(item_expr, &item_ctx)? else {
        return Ok(raw_expr(clean_expr(
            &print_expr(item_expr, &item_ctx)?,
            &item_ctx,
        )));
    };
    rename_node_expr_prefix(&mut item_node, item_param.as_ref(), "item");

    Ok(VueNode::For(VueFor {
        value: "item".to_string(),
        source,
        node: Box::new(item_node),
    }))
}

fn recover_component_vnode(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(component_arg) = args.first() else {
        return Ok(VueNode::RawExpr("createVNode()".into()));
    };
    let Some(component) = recover_component_tag(component_arg.expr.as_ref(), ctx)? else {
        return Ok(raw_expr(clean_expr(
            &print_expr(component_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let mut attrs = component.attrs;
    attrs.extend(
        args.get(1)
            .map(|arg| recover_component_attrs(arg.expr.as_ref(), ctx))
            .transpose()?
            .unwrap_or_default(),
    );
    let children = args
        .get(2)
        .map(|arg| recover_component_children(arg.expr.as_ref(), ctx))
        .transpose()?
        .unwrap_or_default();

    Ok(VueNode::Element(
        VueElement::new(component.tag)
            .with_attrs(attrs)
            .with_children(children),
    ))
}

fn recover_component_children(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueNode>> {
    match expr {
        Expr::Call(call) if helper_name(&call.callee, ctx) == Some(VueHelper::CreateSlots) => {
            recover_create_slots(&call.args, ctx)?
                .map(Ok)
                .unwrap_or_else(|| recover_children(expr, ctx))
        }
        Expr::Object(object) => recover_component_slots(object, ctx)?
            .map(Ok)
            .unwrap_or_else(|| recover_children(expr, ctx)),
        _ => recover_children(expr, ctx),
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
        let Some(slot) = recover_component_slot(&slot_name, key_value.value.as_ref(), ctx)? else {
            return Ok(None);
        };
        slots.push(slot);
    }
    Ok(Some(slots))
}

fn recover_dynamic_slot(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    match expr {
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
        _ => Ok(None),
    }
}

fn recover_slot_descriptor(
    object: &ObjectLit,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    let mut slot_name = None;
    let mut slot_fn = None;

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
                let Some(name) = string_lit(key_value.value.as_ref()) else {
                    return Ok(None);
                };
                slot_name = Some(name);
            }
            "fn" => slot_fn = Some(key_value.value.as_ref()),
            "key" => {}
            _ => return Ok(None),
        }
    }

    let (Some(slot_name), Some(slot_fn)) = (slot_name, slot_fn) else {
        return Ok(None);
    };
    recover_component_slot(&slot_name, slot_fn, ctx)
}

fn recover_component_slot(
    slot_name: &str,
    expr: &Expr,
    ctx: &VueRecoveryContext,
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
    let mut directive = VueDirective::new("slot").with_arg(slot_name.to_string());
    if let Some(scope) = slot_scope(arrow, ctx)? {
        directive = directive.with_expr(scope);
    }
    Ok(Some(VueNode::Element(
        VueElement::new("template")
            .with_attrs(vec![VueAttr::Directive(directive)])
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

fn slot_pat(pat: &Pat, ctx: &VueRecoveryContext) -> Result<Option<String>> {
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

fn is_undefined_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(ident) if ident.sym.as_ref() == "undefined" => true,
        Expr::Lit(Lit::Null(_)) => true,
        Expr::Paren(paren) => is_undefined_expr(paren.expr.as_ref()),
        _ => false,
    }
}

fn recover_text_vnode(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(text_arg) = args.first() else {
        return Ok(VueNode::Text(String::new()));
    };
    if let Some(text) = string_lit(text_arg.expr.as_ref()) {
        return Ok(VueNode::Text(text));
    }
    Ok(raw_expr(clean_expr(
        &print_expr(text_arg.expr.as_ref(), ctx)?,
        ctx,
    )))
}

fn recover_comment_vnode(args: &[ExprOrSpread]) -> Option<VueNode> {
    let comment_arg = args.first()?;
    let comment = string_lit(comment_arg.expr.as_ref())?;
    if comment.is_empty() || comment == "v-if" {
        return None;
    }
    Some(VueNode::Comment(comment))
}

fn recover_slot(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
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

fn recover_with_directives(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(base_arg) = args.first() else {
        return Ok(VueNode::RawExpr("withDirectives()".into()));
    };
    let Some(mut node) = recover_node(base_arg.expr.as_ref(), ctx)? else {
        return Ok(raw_expr(clean_expr(
            &print_expr(base_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Some(directives_arg) = args.get(1) else {
        return Ok(node);
    };
    let Expr::Array(directives) = directives_arg.expr.as_ref() else {
        return Ok(node);
    };

    for directive in directives.elems.iter().flatten() {
        let Some(attr) = recover_directive_tuple(directive.expr.as_ref(), ctx)? else {
            continue;
        };
        push_attr_to_node(&mut node, attr);
    }

    Ok(node)
}

fn recover_with_memo(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(deps_arg) = args.first() else {
        return Ok(VueNode::RawExpr("withMemo()".into()));
    };
    let Some(render_arg) = args.get(1) else {
        return Ok(raw_expr(clean_expr(
            &print_expr(deps_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Expr::Arrow(render_fn) = render_arg.expr.as_ref() else {
        return Ok(raw_expr(clean_expr(
            &print_expr(render_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Some(render_expr) = arrow_return_expr(&render_fn.body) else {
        return Ok(raw_expr(clean_expr(
            &print_expr(render_arg.expr.as_ref(), ctx)?,
            ctx,
        )));
    };
    let Some(mut node) = recover_node(render_expr, ctx)? else {
        return Ok(raw_expr(clean_expr(&print_expr(render_expr, ctx)?, ctx)));
    };
    push_attr_to_node(
        &mut node,
        VueAttr::Directive(VueDirective {
            name: "memo".to_string(),
            arg: None,
            expr: Some(printed_vue_expr(deps_arg.expr.as_ref(), ctx)?),
            modifiers: Vec::new(),
        }),
    );
    Ok(node)
}

fn push_attr_to_node(node: &mut VueNode, attr: VueAttr) {
    match node {
        VueNode::Element(element) => push_attr_to_element(element, attr),
        VueNode::Fragment(children) => {
            if let Some(first) = children.first_mut() {
                push_attr_to_node(first, attr);
            }
        }
        VueNode::If(branches) => {
            if let Some(first) = branches.first_mut() {
                push_attr_to_node(&mut first.node, attr);
            }
        }
        VueNode::For(for_node) => push_attr_to_node(&mut for_node.node, attr),
        VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawExpr(_) => {}
    }
}

fn push_attr_to_element(element: &mut VueElement, attr: VueAttr) {
    if let Some(model_prop) = model_directive_prop(&attr) {
        let update_event = format!("update:{model_prop}");
        element.attrs.retain(
            |existing| !matches!(existing, VueAttr::On { name, .. } if name == &update_event),
        );
    }
    element.attrs.push(attr);
}

fn model_directive_prop(attr: &VueAttr) -> Option<String> {
    let VueAttr::Directive(VueDirective { name, arg, .. }) = attr else {
        return None;
    };
    if name != "model" {
        return None;
    }
    match arg {
        Some(VueDirectiveArg::Static(arg)) => Some(arg.clone()),
        Some(VueDirectiveArg::Dynamic(_)) => None,
        None => Some("modelValue".to_string()),
    }
}

fn arrow_return_expr(body: &BlockStmtOrExpr) -> Option<&Expr> {
    match body {
        BlockStmtOrExpr::Expr(expr) => Some(expr.as_ref()),
        BlockStmtOrExpr::BlockStmt(block) => block.stmts.iter().find_map(|stmt| match stmt {
            Stmt::Return(ReturnStmt {
                arg: Some(expr), ..
            }) => Some(expr.as_ref()),
            _ => None,
        }),
    }
}

fn rename_node_expr_prefix(node: &mut VueNode, from: &str, to: &str) {
    match node {
        VueNode::Element(element) => {
            for attr in &mut element.attrs {
                rename_attr_expr_prefix(attr, from, to);
            }
            for child in &mut element.children {
                rename_node_expr_prefix(child, from, to);
            }
        }
        VueNode::Fragment(children) => {
            for child in children {
                rename_node_expr_prefix(child, from, to);
            }
        }
        VueNode::If(branches) => {
            for branch in branches {
                if let Some(condition) = &mut branch.condition {
                    condition.replace_prefix(from, to);
                }
                rename_node_expr_prefix(&mut branch.node, from, to);
            }
        }
        VueNode::For(for_node) => {
            for_node.source.replace_prefix(from, to);
            rename_node_expr_prefix(&mut for_node.node, from, to);
        }
        VueNode::Interpolation(expr) | VueNode::RawExpr(expr) => expr.replace_prefix(from, to),
        VueNode::Text(_) | VueNode::Comment(_) => {}
    }
}

fn rename_attr_expr_prefix(attr: &mut VueAttr, from: &str, to: &str) {
    match attr {
        VueAttr::Bind { expr, .. }
        | VueAttr::On { expr, .. }
        | VueAttr::Spread(expr)
        | VueAttr::Directive(VueDirective {
            expr: Some(expr), ..
        }) => {
            expr.replace_prefix(from, to);
        }
        VueAttr::Static { .. } | VueAttr::Directive(VueDirective { expr: None, .. }) => {}
    }
}

fn recover_element(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    let Some(tag_arg) = args.first() else {
        return Ok(None);
    };
    if is_fragment_tag(tag_arg.expr.as_ref(), ctx) {
        let children = args
            .get(2)
            .map(|arg| recover_children(arg.expr.as_ref(), ctx))
            .transpose()?
            .unwrap_or_default();
        return Ok(Some(VueNode::Fragment(children)));
    }
    let Some(tag) = string_lit(tag_arg.expr.as_ref()) else {
        return Ok(Some(raw_expr(clean_expr(
            &format!("create element {}", print_expr(tag_arg.expr.as_ref(), ctx)?),
            ctx,
        ))));
    };

    let attrs = args
        .get(1)
        .map(|arg| recover_attrs(arg.expr.as_ref(), ctx))
        .transpose()?
        .unwrap_or_default();

    let children = args
        .get(2)
        .map(|arg| recover_children(arg.expr.as_ref(), ctx))
        .transpose()?
        .unwrap_or_default();

    Ok(Some(VueNode::Element(
        VueElement::new(tag)
            .with_attrs(attrs)
            .with_children(children),
    )))
}

fn clean_condition_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    if let Expr::Paren(paren) = expr {
        return clean_condition_expr(paren.expr.as_ref(), ctx);
    }
    if let Expr::Bin(bin) = expr {
        if matches!(bin.op, BinaryOp::EqEq | BinaryOp::EqEqEq) {
            if let Some(left_str) = string_lit(bin.left.as_ref()) {
                let right = clean_attr_expr(&print_expr(bin.right.as_ref(), ctx)?, ctx);
                return Ok(format!("{right} === '{}'", left_str.replace('\'', "\\'")));
            }
            if let Some(right_str) = string_lit(bin.right.as_ref()) {
                let left = clean_attr_expr(&print_expr(bin.left.as_ref(), ctx)?, ctx);
                return Ok(format!("{left} === '{}'", right_str.replace('\'', "\\'")));
            }
        }
    }
    Ok(clean_attr_expr(&print_expr(expr, ctx)?, ctx))
}

fn recover_children(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueNode>> {
    match expr {
        Expr::Lit(Lit::Null(_)) => Ok(Vec::new()),
        Expr::Lit(Lit::Str(str)) => Ok(vec![VueNode::Text(wtf8_to_string(&str.value))]),
        Expr::Array(array) => {
            let mut children = Vec::new();
            for elem in array.elems.iter().flatten() {
                if let Some(child) = recover_node(elem.expr.as_ref(), ctx)? {
                    children.push(child);
                }
            }
            Ok(children)
        }
        _ => recover_node(expr, ctx).map(|node| node.into_iter().collect()),
    }
}

struct RecoveredComponentTag {
    tag: String,
    attrs: Vec<VueAttr>,
}

fn recover_component_tag(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<RecoveredComponentTag>> {
    match expr {
        Expr::Ident(ident) => Ok(ctx
            .component_bindings
            .get(&ident.sym)
            .cloned()
            .or_else(|| {
                ctx.vue_helpers
                    .get(&ident.sym)
                    .and_then(|helper| match helper {
                        VueHelper::Other(name) if is_builtin_component(name) => Some(name.clone()),
                        _ => None,
                    })
            })
            .or_else(|| is_pascal_case(&ident.sym).then(|| ident.sym.to_string()))
            .map(|tag| RecoveredComponentTag {
                tag,
                attrs: Vec::new(),
            })),
        Expr::Call(call)
            if helper_name(&call.callee, ctx) == Some(VueHelper::ResolveDynamicComponent) =>
        {
            let Some(target) = call.args.first() else {
                return Ok(None);
            };
            Ok(Some(RecoveredComponentTag {
                tag: "component".to_string(),
                attrs: vec![VueAttr::Bind {
                    name: "is".to_string(),
                    expr: printed_vue_expr(target.expr.as_ref(), ctx)?,
                }],
            }))
        }
        _ => Ok(None),
    }
}

fn is_pascal_case(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| ch.is_ascii_uppercase())
        .unwrap_or(false)
}

fn is_builtin_component(name: &str) -> bool {
    matches!(
        name,
        "KeepAlive" | "Suspense" | "Teleport" | "Transition" | "TransitionGroup"
    )
}
