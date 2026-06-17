use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use swc_core::atoms::Atom;
use swc_core::common::{sync::Lrc, FileName, SourceMap, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, BinaryOp, BlockStmtOrExpr, CallExpr, Callee, Decl, ExportDecl, Expr, ExprOrSpread,
    FnDecl, Lit, Module, ModuleDecl, ModuleItem, ObjectLit, Prop, PropOrSpread, ReturnStmt, Stmt,
};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_core::ecma::visit::{Visit, VisitWith};

use crate::driver::{decompile, DecompileOptions, DecompileOutput};
use crate::vue_template::{
    VueAttr, VueDirective, VueDirectiveArg, VueElement, VueExpr, VueFor, VueIfBranch, VueNode,
    VueSfc, VueTemplate,
};

mod context;
mod directives;
mod expressions;
mod helpers;
mod syntax;

use context::{collect_context, collect_render_context, render_context_param};
use directives::{directive_modifiers, recover_directive_tuple};
use expressions::{
    clean_attr_expr, clean_expr, clean_vue_expr, print_expr, printed_vue_expr, raw_expr,
};
use helpers::{helper_call_name, helper_name, is_fragment_tag, VueHelper};
use syntax::{pat_binding_ident, prop_name, string_lit, wtf8_to_string};

#[derive(Default, Clone)]
struct VueRecoveryContext {
    vue_helpers: HashMap<Atom, VueHelper>,
    object_bindings: HashMap<Atom, ObjectLit>,
    component_bindings: HashMap<Atom, String>,
    directive_bindings: HashMap<Atom, String>,
    component_options: Option<ObjectLit>,
    render_context: Option<Atom>,
    cm: Lrc<SourceMap>,
}

pub fn recover_vue_sfc_source_from_js(source: &str) -> Result<Option<String>> {
    Ok(recover_vue_sfc_from_js(source)?.map(|sfc| sfc.print()))
}

pub fn decompile_vue_sfc(source: &str, options: DecompileOptions) -> Result<DecompileOutput> {
    let mut output = decompile(source, options)?;
    if let Some(sfc) = recover_vue_sfc_source_from_js(&output.code)? {
        output.code = sfc;
    }
    Ok(output)
}

pub fn recover_vue_sfc_from_js(source: &str) -> Result<Option<VueSfc>> {
    let cm: Lrc<SourceMap> = Default::default();
    let module = parse_module(source, cm.clone())?;
    let mut ctx = collect_context(&module, cm);
    let Some(render) = find_render_fn(&module) else {
        return Ok(None);
    };
    ctx.render_context = render_context_param(render);
    collect_render_context(render, &mut ctx);
    if !render_uses_vue_helper(render, &ctx) {
        return Ok(None);
    }
    let Some(root) = recover_render_root(render, &ctx)? else {
        return Ok(None);
    };

    let script = ctx
        .component_options
        .as_ref()
        .and_then(|options| component_script(options, &ctx).transpose())
        .transpose()?;

    Ok(Some(VueSfc {
        script,
        template: VueTemplate {
            children: vec![root],
        },
    }))
}

fn parse_module(source: &str, cm: Lrc<SourceMap>) -> Result<Module> {
    let fm = cm.new_source_file(
        FileName::Custom("vue-recovery.js".into()).into(),
        source.to_string(),
    );
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|error| anyhow!("failed to parse decompiled Vue module: {error:?}"))
}

fn find_render_fn(module: &Module) -> Option<&FnDecl> {
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(ExportDecl {
                decl: Decl::Fn(fn_decl),
                ..
            })) if fn_decl.ident.sym.as_ref() == "render" => return Some(fn_decl),
            ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl)))
                if fn_decl.ident.sym.as_ref() == "render" =>
            {
                return Some(fn_decl);
            }
            _ => {}
        }
    }
    None
}

fn render_uses_vue_helper(render: &FnDecl, ctx: &VueRecoveryContext) -> bool {
    let Some(body) = render.function.body.as_ref() else {
        return false;
    };
    if ctx.vue_helpers.is_empty() {
        return false;
    }

    struct Finder<'a> {
        helpers: &'a HashMap<Atom, VueHelper>,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Ident(ident) = callee.as_ref() {
                    if self.helpers.contains_key(&ident.sym) {
                        self.found = true;
                        return;
                    }
                }
            }

            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        helpers: &ctx.vue_helpers,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

fn recover_render_root(render: &FnDecl, ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
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
                VueHelper::CreateTextVNode => recover_text_vnode(&call.args, ctx).map(Some),
                VueHelper::RenderSlot => recover_slot(&call.args, ctx).map(Some),
                VueHelper::RenderList => recover_render_list(&call.args, ctx).map(Some),
                VueHelper::WithDirectives => recover_with_directives(&call.args, ctx).map(Some),
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
        .map(|arg| recover_children(arg.expr.as_ref(), ctx))
        .transpose()?
        .unwrap_or_default();

    Ok(VueNode::Element(
        VueElement::new(component.tag)
            .with_attrs(attrs)
            .with_children(children),
    ))
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

fn push_attr_to_node(node: &mut VueNode, attr: VueAttr) {
    match node {
        VueNode::Element(element) => element.attrs.push(attr),
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

fn recover_attrs(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueAttr>> {
    match expr {
        Expr::Lit(Lit::Null(_)) => Ok(Vec::new()),
        Expr::Object(object) => recover_attrs_from_object(object, ctx),
        Expr::Ident(ident) => {
            if let Some(object) = ctx.object_bindings.get(&ident.sym) {
                recover_attrs_from_object(object, ctx)
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

fn recover_component_attrs(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Vec<VueAttr>> {
    let model_modifiers = match expr {
        Expr::Object(object) => component_model_modifiers(object),
        _ => HashMap::new(),
    };
    Ok(collapse_component_model_attrs(
        recover_attrs(expr, ctx)?,
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

fn recover_attrs_from_object(object: &ObjectLit, ctx: &VueRecoveryContext) -> Result<Vec<VueAttr>> {
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
                    attrs.extend(attrs_from_key_value(&name, key_value.value.as_ref(), ctx)?);
                }
                Prop::Shorthand(ident) => attrs.push(VueAttr::Bind {
                    name: ident.sym.to_string(),
                    expr: VueExpr::new(ident.sym.to_string()),
                }),
                _ => {}
            },
        }
    }
    Ok(attrs)
}

fn attrs_from_key_value(
    name: &str,
    value: &Expr,
    ctx: &VueRecoveryContext,
) -> Result<Vec<VueAttr>> {
    if let Some(event_name) = name.strip_prefix("on").filter(|s| !s.is_empty()) {
        let event = recover_event_expr(value, ctx)?;
        return Ok(vec![VueAttr::On {
            name: lower_first(event_name),
            expr: event.expr,
            modifiers: event.modifiers,
        }]);
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
            arrow_handler_name(assign.right.as_ref(), ctx)
        }
        Expr::Arrow(arrow) => arrow_body_handler_name(&arrow.body, ctx),
        _ => Ok(None),
    }
}

fn arrow_handler_name(body: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match body {
        Expr::Arrow(arrow) => arrow_body_handler_name(&arrow.body, ctx),
        _ => Ok(None),
    }
}

fn arrow_body_handler_name(
    body: &BlockStmtOrExpr,
    ctx: &VueRecoveryContext,
) -> Result<Option<String>> {
    let BlockStmtOrExpr::Expr(expr) = body else {
        return Ok(None);
    };
    logical_handler_name(expr.as_ref(), ctx)
}

fn logical_handler_name(expr: &Expr, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    match expr {
        Expr::Paren(paren) => logical_handler_name(paren.expr.as_ref(), ctx),
        Expr::Bin(bin) if bin.op == BinaryOp::LogicalAnd => Ok(Some(clean_attr_expr(
            &print_expr(bin.left.as_ref(), ctx)?,
            ctx,
        ))),
        _ => Ok(None),
    }
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

fn helper_first_arg_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let Expr::Call(call) = expr else {
        return Ok(clean_expr(&print_expr(expr, ctx)?, ctx));
    };
    let Some(first) = call.args.first() else {
        return Ok(clean_expr(&print_expr(expr, ctx)?, ctx));
    };
    Ok(clean_attr_expr(&print_expr(first.expr.as_ref(), ctx)?, ctx))
}

fn component_script(options: &ObjectLit, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    if options.props.is_empty() {
        return Ok(None);
    }
    let printed = print_expr(&Expr::Object(options.clone()), ctx)?;
    Ok(Some(format!("export default {printed}")))
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_ascii_lowercase().to_string() + chars.as_str()
}

fn is_pascal_case(value: &str) -> bool {
    value
        .chars()
        .next()
        .map(|ch| ch.is_ascii_uppercase())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_plain_render_function_without_vue_signal() {
        let input = r#"
export function render() {
  return "not a Vue render";
}
"#;

        assert!(recover_vue_sfc_source_from_js(input).unwrap().is_none());
    }

    #[test]
    fn ignores_vue_import_without_render_helper_call() {
        let input = r#"
import { ref } from "vue";
const __sfc__ = { props: { msg: String } };
export function render() {
  return "not a Vue render";
}
"#;

        assert!(recover_vue_sfc_source_from_js(input).unwrap().is_none());
    }

    #[test]
    fn recovers_aliased_vue_helper_signal() {
        let input = r#"
import { openBlock as o, createElementBlock as h } from "vue";
export function render(_ctx, _cache) {
  return o(), h("main", null, "Aliased");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <main>Aliased</main>\n</template>\n"
        );
    }

    #[test]
    fn decompiles_then_recovers_vue_sfc() {
        let input = r#"
import { toDisplayString as _toDisplayString, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  return (_openBlock(), _createElementBlock("div", null, _toDisplayString(_ctx.msg), 1));
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            decompile_vue_sfc(input, DecompileOptions::default())
                .unwrap()
                .code,
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_static_element_with_hoisted_props() {
        let input = r#"
import { openBlock, createElementBlock } from "vue";
const __sfc__ = {};
const _hoisted_1 = { class: "card" };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("section", _hoisted_1, "Hello Vue");
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section class=\"card\">Hello Vue</section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_interpolation_and_component_options() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { msg: String } };
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(_ctx.msg), 1);
}
__sfc__.render = render;
export default __sfc__;
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        msg: String\n    }\n}\n</script>\n\n<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_minified_render_context_interpolation() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const e = { props: { msg: String } };
export function render(e, o) {
  openBlock();
  return createElementBlock("div", null, toDisplayString(e.msg), 1);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div>{{ msg }}</div>\n</template>\n"
        );
    }

    #[test]
    fn recovers_class_binding_and_event_handler() {
        let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = {};
export function render(_ctx, _cache) {
  openBlock();
  return createElementBlock("button", {
    class: normalizeClass({ active: props.active }),
    onClick: increment
  }, toDisplayString(props.count), 3);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <button :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_event_handler_modifiers() {
        let input = r#"
import { withKeys, withModifiers, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("input", {
    onKeyup: withKeys(withModifiers(_cache[0] || (_cache[0] = (...args) => (_ctx.submit && _ctx.submit(...args))), ["stop", "prevent"]), ["enter"])
  }, null, 40));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <input @keyup.enter.stop.prevent=\"submit\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_vue_cached_event_and_class_array() {
        let input = r#"
import { toDisplayString, normalizeClass, openBlock, createElementBlock } from "vue";
const __sfc__ = { props: { active: Boolean, count: Number } };
export function render(_ctx, _cache) {
  return (openBlock(), createElementBlock("button", {
    class: normalizeClass(["counter", { active: _ctx.props.active }]),
    onClick: _cache[0] || (_cache[0] = (...args) => (_ctx.increment && _ctx.increment(...args)))
  }, toDisplayString(_ctx.props.count), 3));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<script>\nexport default {\n    props: {\n        active: Boolean,\n        count: Number\n    }\n}\n</script>\n\n<template>\n  <button class=\"counter\" :class=\"{ active: props.active }\" @click=\"increment\">{{ props.count }}</button>\n</template>\n"
        );
    }

    #[test]
    fn recovers_conditional_branch_chain() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  return (_ctx.status === 'loading')
    ? (openBlock(), createElementBlock("p", _hoisted_1, "Loading"))
    : (_ctx.status === 'error')
      ? (openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1))
      : (openBlock(), createElementBlock("p", _hoisted_3, "Ready"));
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\" :key=\"0\">Loading</p>\n  <p v-else-if=\"status === 'error'\" :key=\"1\">{{ error }}</p>\n  <p v-else :key=\"2\">Ready</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_decompiled_if_return_branch_chain() {
        let input = r#"
import { toDisplayString, openBlock, createElementBlock } from "vue";
const _hoisted_1 = { key: 0 };
const _hoisted_2 = { key: 1 };
const _hoisted_3 = { key: 2 };
export function render(_ctx, _cache) {
  if (_ctx.status === "loading") {
    return openBlock(), createElementBlock("p", _hoisted_1, "Loading");
  }
  if (_ctx.status === 'error') {
    return openBlock(), createElementBlock("p", _hoisted_2, toDisplayString(_ctx.error), 1);
  }
  return openBlock(), createElementBlock("p", _hoisted_3, "Ready");
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <p v-if=\"status === 'loading'\" :key=\"0\">Loading</p>\n  <p v-else-if=\"status === 'error'\" :key=\"1\">{{ error }}</p>\n  <p v-else :key=\"2\">Ready</p>\n</template>\n"
        );
    }

    #[test]
    fn recovers_render_list_fragment_with_mangled_item_param() {
        let input = r#"
import { renderList as r, Fragment as t, openBlock as n, createElementBlock as o, toDisplayString as s } from "vue";
export function render(e, a) {
  return n(), o("ul", null, [
    (n(true), o(t, null, r(e.items, e => (n(), o("li", { key: e.id }, s(e.name), 1))), 128))
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <ul>\n    <li v-for=\"item in items\" :key=\"item.id\">{{ item.name }}</li>\n  </ul>\n</template>\n"
        );
    }

    #[test]
    fn recovers_component_vnode_and_named_slot() {
        let input = r#"
import { resolveComponent, createVNode, renderSlot, createTextVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_PanelHeader = resolveComponent("PanelHeader");
  return openBlock(), createElementBlock("article", null, [
    createVNode(_component_PanelHeader, { title: _ctx.title }, null, 8, ["title"]),
    renderSlot(_ctx.$slots, "body", {}, () => [
      _cache[0] || (_cache[0] = createTextVNode("Empty", -1))
    ])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <article>\n    <PanelHeader :title=\"title\" />\n    <slot name=\"body\">Empty</slot>\n  </article>\n</template>\n"
        );
    }

    #[test]
    fn recovers_component_v_model_pairs() {
        let input = r#"
import { resolveComponent, createVNode, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _component_FormInput = resolveComponent("FormInput");
  return openBlock(), createElementBlock("section", null, [
    createVNode(_component_FormInput, {
      modelValue: _ctx.name,
      "onUpdate:modelValue": $event => _ctx.name = $event,
      modelModifiers: { trim: true },
      filter: _ctx.filter,
      "onUpdate:filter": $event => _ctx.filter = $event,
      filterModifiers: { number: true, lazy: true },
      label: "Name"
    }, null, 8, ["modelValue", "filter"])
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <section>\n    <FormInput v-model.trim=\"name\" v-model:filter.number.lazy=\"filter\" label=\"Name\" />\n  </section>\n</template>\n"
        );
    }

    #[test]
    fn recovers_dynamic_component() {
        let input = r#"
import { resolveDynamicComponent, openBlock, createBlock } from "vue";
export function render(_ctx, _cache) {
  return openBlock(), createBlock(resolveDynamicComponent(_ctx.currentView), {
    class: "panel"
  }, null, 512);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <component :is=\"currentView\" class=\"panel\" />\n</template>\n"
        );
    }

    #[test]
    fn recovers_model_and_show_directives() {
        let input = r#"
import { vModelText, vShow, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  return withDirectives((openBlock(), createElementBlock("input", {
    "onUpdate:modelValue": _cache[0] || (_cache[0] = $event => _ctx.value = $event)
  }, null, 512)), [
    [vModelText, _ctx.value, void 0, { trim: true, number: true }],
    [vShow, _ctx.visible]
  ]);
}
"#;

        let output = recover_vue_sfc_source_from_js(input).unwrap().unwrap();
        assert!(output.contains("v-model.trim.number=\"value\""));
        assert!(output.contains("v-show=\"visible\""));
    }

    #[test]
    fn recovers_custom_directive_payload() {
        let input = r#"
import { resolveDirective, withDirectives, openBlock, createElementBlock } from "vue";
export function render(_ctx, _cache) {
  const _directive_focus = resolveDirective("focus");
  return withDirectives((openBlock(), createElementBlock("div", null, null, 512)), [
    [_directive_focus, _ctx.value, "current", { trim: true, deep: true }]
  ]);
}
"#;

        assert_eq!(
            recover_vue_sfc_source_from_js(input).unwrap().unwrap(),
            "<template>\n  <div v-focus:current.trim.deep=\"value\" />\n</template>\n"
        );
    }
}
