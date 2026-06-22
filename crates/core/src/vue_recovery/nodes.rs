use anyhow::Result;
use swc_core::atoms::Atom;
use swc_core::ecma::ast::{
    AssignOp, BinaryOp, BlockStmtOrExpr, Callee, Expr, ExprOrSpread, Lit, MemberProp,
    ObjectPatProp, Pat, ReturnStmt, Stmt, Tpl, UnaryOp,
};

use super::attrs::{recover_attrs, recover_component_attrs};
use super::directives::recover_directive_tuple;
use super::expressions::{
    clean_attr_expr, clean_expr, print_expr, printed_vue_expr, raw_expr,
    unsupported_vnode_children_expr,
};
use super::helpers::{helper_name, is_fragment_tag, VueHelper};
use super::slots::{
    recover_component_children as recover_slot_component_children, recover_direct_slot,
    recover_slot, slot_pat,
};
use super::syntax::{string_lit, wtf8_to_string};
use super::{RenderSource, VueRecoveryContext, VueRenderChildListSource};
use crate::vue_template::{
    VueAttr, VueDirective, VueDirectiveArg, VueElement, VueExpr, VueFor, VueIfBranch, VueNode,
    VueTemplateScope,
};

pub(super) fn recover_render_root(
    render: RenderSource<'_>,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    if let Some(stmts) = render_stmts(render) {
        if let Some(node) = recover_render_if_chain(stmts, ctx)? {
            return Ok(Some(node));
        }
    }
    let Some(root_expr) = find_render_return(render) else {
        return Ok(None);
    };
    recover_node(root_expr, ctx)
}

fn recover_render_if_chain(stmts: &[Stmt], ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    let mut branches = Vec::new();
    let mut in_chain = false;

    for stmt in stmts {
        match stmt {
            Stmt::If(if_stmt) => {
                let Some(expr) = return_expr_from_stmt(if_stmt.cons.as_ref()) else {
                    return Ok(None);
                };
                let Some(mut node) = recover_node(expr, ctx)? else {
                    continue;
                };
                strip_generated_branch_key(&mut node);
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
                if let Some(mut node) = recover_node(expr.as_ref(), ctx)? {
                    strip_generated_branch_key(&mut node);
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

fn find_render_return(render: RenderSource<'_>) -> Option<&Expr> {
    match render {
        RenderSource::Function(render) => {
            let body = render.function.body.as_ref()?;
            body.stmts.iter().rev().find_map(|stmt| match stmt {
                Stmt::Return(ReturnStmt {
                    arg: Some(expr), ..
                }) => Some(expr.as_ref()),
                _ => None,
            })
        }
        RenderSource::SetupArrow { render, .. } => arrow_return_expr(&render.body),
    }
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
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => {
            recover_logical_and_node(bin.left.as_ref(), bin.right.as_ref(), ctx)
        }
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
        Expr::Tpl(tpl) => recover_template_literal(tpl, ctx).map(Some),
        Expr::Call(call) => {
            let Some(helper) = helper_name(&call.callee, ctx) else {
                return recover_unsupported_vnode_children(expr, ctx).map(Some);
            };
            match helper {
                VueHelper::CreateElementBlock | VueHelper::CreateElementVNode => {
                    recover_element(&call.args, ctx)
                }
                VueHelper::CreateBlock | VueHelper::CreateVNode
                    if call
                        .args
                        .first()
                        .is_some_and(|arg| string_lit(arg.expr.as_ref()).is_some()) =>
                {
                    recover_element(&call.args, ctx)
                }
                VueHelper::CreateBlock | VueHelper::CreateVNode => {
                    recover_component_vnode(&call.args, ctx).map(Some)
                }
                VueHelper::CreateCommentVNode => Ok(recover_comment_vnode(&call.args)),
                VueHelper::CreateStaticVNode => recover_static_vnode(&call.args, ctx).map(Some),
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
                _ => recover_unsupported_vnode_children(expr, ctx).map(Some),
            }
        }
        Expr::Ident(ident) if ctx.render_child_list_bindings.contains_key(&ident.sym) => {
            recover_unsupported_vnode_children(expr, ctx).map(Some)
        }
        Expr::Member(member) => {
            if let Some(slot) = recover_direct_slot(member, ctx)? {
                return Ok(Some(slot));
            }
            Ok(Some(raw_expr(clean_expr(&print_expr(expr, ctx)?, ctx))))
        }
        Expr::Lit(Lit::Str(str)) => Ok(Some(VueNode::Text(wtf8_to_string(&str.value)))),
        _ => Ok(Some(raw_expr(clean_expr(&print_expr(expr, ctx)?, ctx)))),
    }
}

fn recover_unsupported_vnode_children(expr: &Expr, ctx: &VueRecoveryContext) -> Result<VueNode> {
    if let Some((_binding, source)) = render_child_list_source_expr(expr, ctx) {
        return Ok(render_local_vnode_children_node(source));
    }
    let printed = clean_expr(&print_expr(expr, ctx)?, ctx);
    Ok(unsupported_vnode_children_expr(printed))
}

fn render_child_list_source_expr<'a>(
    expr: &Expr,
    ctx: &'a VueRecoveryContext,
) -> Option<(&'a Atom, VueRenderChildListSource)> {
    match expr {
        Expr::Paren(paren) => render_child_list_source_expr(paren.expr.as_ref(), ctx),
        Expr::Seq(seq) => seq
            .exprs
            .last()
            .and_then(|expr| render_child_list_source_expr(expr.as_ref(), ctx)),
        Expr::Assign(assign) => render_child_list_source_expr(assign.right.as_ref(), ctx),
        Expr::Ident(ident) => ctx
            .render_child_list_bindings
            .get_key_value(&ident.sym)
            .map(|(binding, source)| (binding, source.source)),
        Expr::Call(call) => call
            .args
            .iter()
            .find_map(|arg| render_child_list_source_expr(arg.expr.as_ref(), ctx)),
        _ => None,
    }
}

fn render_local_vnode_children_node(source: VueRenderChildListSource) -> VueNode {
    match source {
        VueRenderChildListSource::SlotPartitionChildren => {
            VueNode::Element(VueElement::new("slot"))
        }
    }
}

fn recover_logical_and_node(
    test: &Expr,
    value: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<VueNode>> {
    let mut node = match value {
        Expr::Array(_) => nodes_to_node(recover_children(value, ctx)?),
        _ => {
            let Some(node) = recover_node(value, ctx)? else {
                return Ok(None);
            };
            node
        }
    };
    strip_generated_branch_key(&mut node);
    Ok(Some(VueNode::If(vec![VueIfBranch {
        condition: Some(VueExpr::new(clean_condition_expr(test, ctx)?)),
        node: Box::new(node),
    }])))
}

fn recover_conditional_chain(
    test: &Expr,
    cons: &Expr,
    alt: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<VueNode> {
    let mut branches = Vec::new();
    let cons_node = recover_node(cons, ctx)?;
    if let Some(mut node) = cons_node {
        strip_generated_branch_key(&mut node);
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
                mut node => {
                    strip_generated_branch_key(&mut node);
                    branches.push(VueIfBranch {
                        condition: None,
                        node: Box::new(node),
                    });
                }
            }
        }
        _ => {
            if let Some(mut node) = recover_node(alt, ctx)? {
                strip_generated_branch_key(&mut node);
                branches.push(VueIfBranch {
                    condition: branches
                        .is_empty()
                        .then(|| clean_negated_condition_expr(test, ctx))
                        .transpose()?
                        .map(VueExpr::new),
                    node: Box::new(node),
                });
            }
        }
    }

    Ok(VueNode::If(branches))
}

fn strip_generated_branch_key(node: &mut VueNode) {
    match node {
        VueNode::Element(element) => {
            element.attrs.retain(|attr| !is_generated_branch_key(attr));
        }
        VueNode::Fragment(children) if children.len() == 1 => {
            strip_generated_branch_key(&mut children[0]);
        }
        VueNode::Fragment(_)
        | VueNode::If(_)
        | VueNode::For(_)
        | VueNode::Text(_)
        | VueNode::Interpolation(_)
        | VueNode::Comment(_)
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
    }
}

fn is_generated_branch_key(attr: &VueAttr) -> bool {
    match attr {
        VueAttr::Bind { name, expr } if name == "key" => {
            !expr.as_str().is_empty() && expr.as_str().chars().all(|ch| ch.is_ascii_digit())
        }
        VueAttr::Static {
            name,
            value: Some(value),
        } if name == "key" => !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit()),
        _ => false,
    }
}

fn recover_render_list(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(source_arg) = args.first() else {
        return Ok(unsupported_vnode_children_expr("renderList()"));
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
    let Some(for_params) = recover_for_params(&callback.params, "item", ctx)? else {
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
    let item_ctx = list_item_context(ctx, &for_params);
    let Some(mut item_node) = recover_node(item_expr, &item_ctx)? else {
        return Ok(raw_expr(clean_expr(
            &print_expr(item_expr, &item_ctx)?,
            &item_ctx,
        )));
    };
    apply_for_param_renames(&mut item_node, &for_params);

    let scope = for_params.template_scope();
    let value = for_params.value;
    Ok(VueNode::For(VueFor {
        value,
        source,
        node: Box::new(item_node),
        scope,
    }))
}

pub(super) struct RecoveredForParams {
    pub(super) value: String,
    renames: Vec<(Atom, String)>,
    bindings: Vec<Atom>,
}

impl RecoveredForParams {
    fn shadows(&self, name: &Atom) -> bool {
        self.bindings.iter().any(|binding| binding == name)
    }

    pub(super) fn template_scope(&self) -> VueTemplateScope {
        VueTemplateScope::from_locals(self.bindings.iter().map(|binding| {
            self.renames
                .iter()
                .find_map(|(from, to)| (from == binding).then(|| to.clone()))
                .unwrap_or_else(|| binding.to_string())
        }))
    }
}

pub(super) fn recover_for_params(
    params: &[Pat],
    first_fallback: &'static str,
    ctx: &VueRecoveryContext,
) -> Result<Option<RecoveredForParams>> {
    if params.is_empty() {
        return Ok(None);
    }

    let fallback_names = for_param_fallback_names(params.len(), first_fallback);
    let mut values = Vec::new();
    let mut renames = Vec::new();
    let mut bindings = Vec::new();

    for (param, fallback) in params.iter().take(3).zip(fallback_names) {
        collect_pat_bindings(param, &mut bindings);
        match param {
            Pat::Ident(binding) => {
                values.push(fallback.to_string());
                renames.push((binding.id.sym.clone(), fallback.to_string()));
            }
            _ => {
                let Some(value) = for_param_pat(param, ctx)? else {
                    return Ok(None);
                };
                values.push(value);
            }
        }
    }

    let value = if values.len() == 1 {
        values.remove(0)
    } else {
        format!("({})", values.join(", "))
    };

    Ok(Some(RecoveredForParams {
        value,
        renames,
        bindings,
    }))
}

fn for_param_fallback_names(count: usize, first: &'static str) -> Vec<&'static str> {
    match count {
        0 => Vec::new(),
        1 => vec![first],
        2 => vec![first, "index"],
        _ => vec![first, "key", "index"],
    }
}

fn for_param_pat(pat: &Pat, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match pat {
        Pat::Array(array) => {
            let mut elems = Vec::new();
            for elem in &array.elems {
                let Some(elem) = elem else {
                    elems.push(String::new());
                    continue;
                };
                let Some(value) = for_param_pat(elem, ctx)? else {
                    return Ok(None);
                };
                elems.push(value);
            }
            Ok(Some(format!("[{}]", elems.join(", "))))
        }
        Pat::Object(_) => slot_pat(pat, ctx),
        Pat::Ident(binding) => Ok(Some(binding.id.sym.to_string())),
        _ => Ok(None),
    }
}

pub(super) fn apply_for_param_renames(node: &mut VueNode, params: &RecoveredForParams) {
    for (from, to) in &params.renames {
        rename_node_expr_prefix(node, from.as_ref(), to);
    }
}

pub(super) fn list_item_context(
    ctx: &VueRecoveryContext,
    params: &RecoveredForParams,
) -> VueRecoveryContext {
    let mut item_ctx = ctx.clone();
    if item_ctx
        .render_context
        .as_ref()
        .is_some_and(|name| params.shadows(name))
    {
        item_ctx.render_context = None;
    }
    if item_ctx
        .setup_props_context
        .as_ref()
        .is_some_and(|name| params.shadows(name))
    {
        item_ctx.setup_props_context = None;
    }
    item_ctx
        .setup_props_aliases
        .retain(|alias| !params.shadows(alias));
    item_ctx
        .bindings
        .refs
        .retain(|binding| !params.shadows(binding));
    item_ctx
        .bindings
        .template_refs
        .retain(|binding| !params.shadows(binding));
    item_ctx
}

fn collect_pat_bindings(pat: &Pat, bindings: &mut Vec<Atom>) {
    match pat {
        Pat::Ident(binding) => bindings.push(binding.id.sym.clone()),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bindings(elem, bindings);
            }
        }
        Pat::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(key_value) => {
                        collect_pat_bindings(key_value.value.as_ref(), bindings);
                    }
                    ObjectPatProp::Assign(assign) => bindings.push(assign.key.sym.clone()),
                    ObjectPatProp::Rest(rest) => collect_pat_bindings(rest.arg.as_ref(), bindings),
                }
            }
        }
        Pat::Assign(assign) => collect_pat_bindings(assign.left.as_ref(), bindings),
        Pat::Expr(_) | Pat::Invalid(_) => {}
    }
}

fn recover_component_vnode(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(component_arg) = args.first() else {
        return Ok(unsupported_vnode_children_expr("createVNode()"));
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

    let mut element = VueElement::new(component.tag).with_attrs(attrs);
    if let Some(import_ref) = component.import_ref {
        element = element.with_component_import_ref(import_ref.to_string());
    }
    Ok(VueNode::Element(element.with_children(children)))
}

fn recover_component_children(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueNode>> {
    recover_slot_component_children(expr, ctx)?
        .map(Ok)
        .unwrap_or_else(|| recover_children(expr, ctx))
}

pub(super) fn is_undefined_expr(expr: &Expr) -> bool {
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
    if let Expr::Tpl(tpl) = text_arg.expr.as_ref() {
        return recover_template_literal(tpl, ctx);
    }
    if let Expr::Call(call) = text_arg.expr.as_ref() {
        if helper_name(&call.callee, ctx) == Some(VueHelper::ToDisplayString) {
            let Some(arg) = call.args.first() else {
                return Ok(VueNode::Interpolation(VueExpr::new(String::new())));
            };
            return Ok(VueNode::Interpolation(VueExpr::new(clean_expr(
                &print_expr(arg.expr.as_ref(), ctx)?,
                ctx,
            ))));
        }
    }
    Ok(raw_expr(clean_expr(
        &print_expr(text_arg.expr.as_ref(), ctx)?,
        ctx,
    )))
}

fn recover_template_literal(tpl: &Tpl, ctx: &VueRecoveryContext) -> Result<VueNode> {
    let mut nodes = Vec::new();
    for (index, quasi) in tpl.quasis.iter().enumerate() {
        let text = quasi
            .cooked
            .as_ref()
            .map(wtf8_to_string)
            .unwrap_or_else(|| quasi.raw.to_string());
        if keep_template_text(&text, index, tpl.quasis.len()) {
            nodes.push(VueNode::Text(text));
        }
        if let Some(expr) = tpl.exprs.get(index) {
            nodes.push(recover_template_literal_expr(expr.as_ref(), ctx)?);
        }
    }
    Ok(nodes_to_node(nodes))
}

fn recover_template_literal_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<VueNode> {
    let expr = if let Expr::Call(call) = expr {
        if helper_name(&call.callee, ctx) == Some(VueHelper::ToDisplayString) {
            call.args
                .first()
                .map(|arg| arg.expr.as_ref())
                .unwrap_or(expr)
        } else {
            expr
        }
    } else {
        expr
    };

    Ok(VueNode::Interpolation(VueExpr::new(clean_expr(
        &print_expr(expr, ctx)?,
        ctx,
    ))))
}

fn keep_template_text(text: &str, index: usize, quasi_count: usize) -> bool {
    !text.is_empty() && !(text.trim().is_empty() && (index == 0 || index + 1 == quasi_count))
}

fn nodes_to_node(mut nodes: Vec<VueNode>) -> VueNode {
    if nodes.len() == 1 {
        nodes.remove(0)
    } else {
        VueNode::Fragment(nodes)
    }
}

fn recover_static_vnode(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(html_arg) = args.first() else {
        return Ok(unsupported_vnode_children_expr("createStaticVNode()"));
    };
    if let Some(html) = string_lit(html_arg.expr.as_ref()) {
        return Ok(VueNode::RawHtml(html));
    }
    Ok(raw_expr(clean_expr(
        &print_expr(html_arg.expr.as_ref(), ctx)?,
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

fn recover_with_directives(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<VueNode> {
    let Some(base_arg) = args.first() else {
        return Ok(unsupported_vnode_children_expr("withDirectives()"));
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
        return Ok(unsupported_vnode_children_expr("withMemo()"));
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
            scope: Default::default(),
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
        | VueNode::RawHtml(_)
        | VueNode::RawExpr(_)
        | VueNode::Unsupported(_) => {}
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

pub(super) fn arrow_return_expr(body: &BlockStmtOrExpr) -> Option<&Expr> {
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
        VueNode::Unsupported(unsupported) => unsupported.expr.replace_prefix(from, to),
        VueNode::Text(_) | VueNode::Comment(_) | VueNode::RawHtml(_) => {}
    }
}

fn rename_attr_expr_prefix(attr: &mut VueAttr, from: &str, to: &str) {
    match attr {
        VueAttr::Bind { expr, .. } | VueAttr::On { expr, .. } | VueAttr::Spread(expr) => {
            expr.replace_prefix(from, to);
        }
        VueAttr::Directive(directive) => {
            if let Some(VueDirectiveArg::Dynamic(arg)) = &mut directive.arg {
                arg.replace_prefix(from, to);
            }
            if let Some(expr) = &mut directive.expr {
                expr.replace_prefix(from, to);
            }
        }
        VueAttr::Static { .. } => {}
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

pub(super) fn clean_condition_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
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

fn clean_negated_condition_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    if let Expr::Paren(paren) = expr {
        return clean_negated_condition_expr(paren.expr.as_ref(), ctx);
    }
    if let Expr::Unary(unary) = expr {
        if unary.op == UnaryOp::Bang {
            return clean_condition_expr(unary.arg.as_ref(), ctx);
        }
    }

    let condition = clean_condition_expr(expr, ctx)?;
    if can_prefix_not(&condition) {
        Ok(format!("!{condition}"))
    } else {
        Ok(format!("!({condition})"))
    }
}

fn can_prefix_not(condition: &str) -> bool {
    !condition.is_empty()
        && condition
            .chars()
            .all(|ch| ch == '_' || ch == '$' || ch == '.' || ch.is_ascii_alphanumeric())
}

pub(super) fn recover_children(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueNode>> {
    match expr {
        Expr::Lit(Lit::Null(_)) => Ok(Vec::new()),
        Expr::Lit(Lit::Str(str)) => Ok(vec![VueNode::Text(wtf8_to_string(&str.value))]),
        Expr::Array(array) => {
            let mut children = Vec::new();
            for elem in array.elems.iter().flatten() {
                if let Some(child) = recover_node(elem.expr.as_ref(), ctx)? {
                    push_child(&mut children, child);
                }
            }
            Ok(children)
        }
        _ => recover_node(expr, ctx).map(recovered_node_children),
    }
}

fn recovered_node_children(node: Option<VueNode>) -> Vec<VueNode> {
    match node {
        Some(VueNode::Fragment(children)) => children,
        Some(node) => vec![node],
        None => Vec::new(),
    }
}

fn push_child(children: &mut Vec<VueNode>, child: VueNode) {
    match child {
        VueNode::Fragment(grandchildren) => children.extend(grandchildren),
        child => children.push(child),
    }
}

struct RecoveredComponentTag {
    tag: String,
    import_ref: Option<Atom>,
    attrs: Vec<VueAttr>,
}

fn recover_component_tag(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<RecoveredComponentTag>> {
    if let Some(component) = recover_static_component_tag(expr, ctx) {
        return Ok(Some(component));
    }

    match expr {
        Expr::Ident(_) => recover_dynamic_component_tag(expr, ctx),
        Expr::Call(call)
            if helper_name(&call.callee, ctx) == Some(VueHelper::ResolveDynamicComponent) =>
        {
            let Some(target) = call.args.first() else {
                return Ok(None);
            };
            if let Some(component) = recover_static_component_tag(target.expr.as_ref(), ctx) {
                return Ok(Some(component));
            }
            Ok(Some(RecoveredComponentTag {
                tag: "component".to_string(),
                import_ref: None,
                attrs: vec![VueAttr::Bind {
                    name: "is".to_string(),
                    expr: printed_vue_expr(target.expr.as_ref(), ctx)?,
                }],
            }))
        }
        Expr::Call(call) if is_vue_helper_candidate_call(call, ctx) => {
            let Some(target) = call.args.first() else {
                return Ok(None);
            };
            if let Some(component) = recover_static_component_tag(target.expr.as_ref(), ctx) {
                return Ok(Some(component));
            }
            recover_dynamic_component_tag(target.expr.as_ref(), ctx)
        }
        _ => recover_dynamic_component_tag(expr, ctx),
    }
}

fn recover_static_component_tag(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Option<RecoveredComponentTag> {
    let ident = match expr {
        Expr::Ident(ident) => ident,
        Expr::Member(member) if is_default_member_prop(&member.prop) => {
            let Expr::Ident(ident) = member.obj.as_ref() else {
                return None;
            };
            ident
        }
        _ => return None,
    };

    let import_ref = ctx
        .script_imports
        .contains_key(&ident.sym)
        .then(|| ident.sym.clone());

    ctx.component_bindings
        .get(&ident.sym)
        .cloned()
        .map(|tag| RecoveredComponentTag {
            tag,
            import_ref: import_ref.clone(),
            attrs: Vec::new(),
        })
        .or_else(|| {
            ctx.vue_helpers
                .get(&ident.sym)
                .and_then(|helper| match helper {
                    VueHelper::Other(name) if is_builtin_component(name) => {
                        Some(RecoveredComponentTag {
                            tag: name.clone(),
                            import_ref: None,
                            attrs: Vec::new(),
                        })
                    }
                    _ => None,
                })
        })
        .or_else(|| {
            is_pascal_case(&ident.sym).then(|| RecoveredComponentTag {
                tag: ident.sym.to_string(),
                import_ref,
                attrs: Vec::new(),
            })
        })
}

fn is_default_member_prop(prop: &MemberProp) -> bool {
    match prop {
        MemberProp::Ident(ident) => ident.sym.as_ref() == "default",
        MemberProp::Computed(computed) => {
            string_lit(computed.expr.as_ref()).as_deref() == Some("default")
        }
        MemberProp::PrivateName(_) => false,
    }
}

fn recover_dynamic_component_tag(
    expr: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Option<RecoveredComponentTag>> {
    if !is_dynamic_component_target(expr, ctx) {
        return Ok(None);
    }
    Ok(Some(RecoveredComponentTag {
        tag: "component".to_string(),
        import_ref: None,
        attrs: vec![VueAttr::Bind {
            name: "is".to_string(),
            expr: printed_vue_expr(expr, ctx)?,
        }],
    }))
}

fn is_dynamic_component_target(expr: &Expr, ctx: &VueRecoveryContext) -> bool {
    match expr {
        Expr::Paren(paren) => is_dynamic_component_target(paren.expr.as_ref(), ctx),
        Expr::Ident(ident) => !ctx.vue_helpers.contains_key(&ident.sym),
        Expr::Member(_)
        | Expr::OptChain(_)
        | Expr::Call(_)
        | Expr::Cond(_)
        | Expr::Bin(_)
        | Expr::Seq(_) => true,
        _ => false,
    }
}

fn is_vue_helper_candidate_call(
    call: &swc_core::ecma::ast::CallExpr,
    ctx: &VueRecoveryContext,
) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    let Expr::Ident(ident) = callee.as_ref() else {
        return false;
    };
    ctx.vue_helper_candidates.contains(&ident.sym)
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
