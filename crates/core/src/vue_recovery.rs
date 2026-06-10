use std::collections::HashMap;

use anyhow::{anyhow, Result};
use swc_core::atoms::{Atom, Wtf8Atom};
use swc_core::common::{sync::Lrc, FileName, SourceMap, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, BinaryOp, BindingIdent, BlockStmtOrExpr, Callee, Decl, ExportDecl, Expr,
    ExprOrSpread, FnDecl, Ident, ImportSpecifier, Lit, Module, ModuleDecl, ModuleExportName,
    ModuleItem, ObjectLit, Param, Pat, Prop, PropName, PropOrSpread, ReturnStmt, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};

use crate::driver::{decompile, DecompileOptions, DecompileOutput};
use crate::vue_template::{VueAttr, VueElement, VueNode, VueSfc, VueTemplate};

#[derive(Default)]
struct VueRecoveryContext {
    vue_helpers: HashMap<Atom, String>,
    object_bindings: HashMap<Atom, ObjectLit>,
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
    let Some(root_expr) = find_render_return(render) else {
        return Ok(None);
    };
    let Some(root) = recover_node(root_expr, &ctx)? else {
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

fn collect_context(module: &Module, cm: Lrc<SourceMap>) -> VueRecoveryContext {
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
                        ctx.vue_helpers.insert(named.local.sym.clone(), imported);
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

fn module_export_name(name: &ModuleExportName) -> String {
    match name {
        ModuleExportName::Ident(ident) => ident.sym.to_string(),
        ModuleExportName::Str(str) => wtf8_to_string(&str.value),
    }
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

fn find_render_return(render: &FnDecl) -> Option<&Expr> {
    let body = render.function.body.as_ref()?;
    body.stmts.iter().rev().find_map(|stmt| match stmt {
        Stmt::Return(ReturnStmt {
            arg: Some(expr), ..
        }) => Some(expr.as_ref()),
        _ => None,
    })
}

fn render_context_param(render: &FnDecl) -> Option<Atom> {
    render
        .function
        .params
        .first()
        .and_then(param_binding_ident)
        .map(|ident| ident.sym.clone())
}

fn param_binding_ident(param: &Param) -> Option<&Ident> {
    match &param.pat {
        Pat::Ident(binding) => Some(&binding.id),
        _ => None,
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
        Expr::Call(call) => {
            let Some(helper) = helper_name(&call.callee, ctx) else {
                return Ok(Some(VueNode::RawExpr(clean_expr(
                    &print_expr(expr, ctx)?,
                    ctx,
                ))));
            };
            match helper.as_str() {
                "createElementBlock" | "createElementVNode" => recover_element(&call.args, ctx),
                "toDisplayString" => {
                    let Some(arg) = call.args.first() else {
                        return Ok(None);
                    };
                    Ok(Some(VueNode::Interpolation(clean_expr(
                        &print_expr(arg.expr.as_ref(), ctx)?,
                        ctx,
                    ))))
                }
                _ => Ok(Some(VueNode::RawExpr(clean_expr(
                    &print_expr(expr, ctx)?,
                    ctx,
                )))),
            }
        }
        Expr::Lit(Lit::Str(str)) => Ok(Some(VueNode::Text(wtf8_to_string(&str.value)))),
        _ => Ok(Some(VueNode::RawExpr(clean_expr(
            &print_expr(expr, ctx)?,
            ctx,
        )))),
    }
}

fn recover_element(args: &[ExprOrSpread], ctx: &VueRecoveryContext) -> Result<Option<VueNode>> {
    let Some(tag_arg) = args.first() else {
        return Ok(None);
    };
    let Some(tag) = string_lit(tag_arg.expr.as_ref()) else {
        return Ok(Some(VueNode::RawExpr(clean_expr(
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
                Ok(vec![VueAttr::Spread(clean_expr(
                    &print_expr(expr, ctx)?,
                    ctx,
                ))])
            }
        }
        _ => Ok(vec![VueAttr::Spread(clean_expr(
            &print_expr(expr, ctx)?,
            ctx,
        ))]),
    }
}

fn recover_attrs_from_object(object: &ObjectLit, ctx: &VueRecoveryContext) -> Result<Vec<VueAttr>> {
    let mut attrs = Vec::new();
    for prop in &object.props {
        match prop {
            PropOrSpread::Spread(spread) => {
                attrs.push(VueAttr::Spread(clean_expr(
                    &print_expr(spread.expr.as_ref(), ctx)?,
                    ctx,
                )));
            }
            PropOrSpread::Prop(prop) => match prop.as_ref() {
                Prop::KeyValue(key_value) => {
                    let Some(name) = prop_name(&key_value.key) else {
                        attrs.push(VueAttr::Spread(clean_expr(
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
                    expr: ident.sym.to_string(),
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
    if let Some(event) = name.strip_prefix("on").filter(|s| !s.is_empty()) {
        return Ok(vec![VueAttr::On {
            name: lower_first(event),
            expr: clean_event_expr(value, ctx)?,
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
            expr: helper_first_arg_expr(value, ctx)?,
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
            expr: clean_attr_expr(&print_expr(value, ctx)?, ctx),
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
            expr: helper_first_arg_expr(value, ctx)?,
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
                expr: clean_attr_expr(&print_expr(elem.expr.as_ref(), ctx)?, ctx),
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

fn clean_event_expr(value: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    if let Some(handler) = cached_event_handler_name(value, ctx)? {
        return Ok(handler);
    }
    Ok(clean_attr_expr(&print_expr(value, ctx)?, ctx))
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

fn helper_call_name(expr: &Expr, ctx: &VueRecoveryContext) -> Option<String> {
    let Expr::Call(call) = expr else {
        return None;
    };
    helper_name(&call.callee, ctx)
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

fn helper_name(callee: &Callee, ctx: &VueRecoveryContext) -> Option<String> {
    let Callee::Expr(expr) = callee else {
        return None;
    };
    match expr.as_ref() {
        Expr::Ident(ident) => ctx.vue_helpers.get(&ident.sym).cloned(),
        _ => None,
    }
}

fn component_script(options: &ObjectLit, ctx: &VueRecoveryContext) -> Result<Option<String>> {
    if options.props.is_empty() {
        return Ok(None);
    }
    let printed = print_expr(&Expr::Object(options.clone()), ctx)?;
    Ok(Some(format!("export default {printed}")))
}

fn print_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind: VarDeclKind::Const,
            declare: false,
            decls: vec![VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: Ident::new("__wakaru_expr".into(), DUMMY_SP, Default::default()),
                    type_ann: None,
                }),
                init: Some(Box::new(expr.clone())),
                definite: false,
            }],
        }))))],
        shebang: None,
    };
    let mut output = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: Config::default().with_minify(false),
            cm: ctx.cm.clone(),
            comments: None,
            wr: JsWriter::new(ctx.cm.clone(), "\n", &mut output, None),
        };
        emitter
            .emit_module(&module)
            .map_err(|error| anyhow!("failed to print Vue expression: {error:?}"))?;
    }
    let code = String::from_utf8(output)
        .map(|s| s.trim().to_string())
        .map_err(|error| anyhow!("printed Vue expression is not UTF-8: {error}"))?;
    Ok(code
        .strip_prefix("const __wakaru_expr = ")
        .unwrap_or(&code)
        .trim_end_matches(';')
        .trim()
        .to_string())
}

fn clean_expr(expr: &str, ctx: &VueRecoveryContext) -> String {
    let mut cleaned = expr
        .replace("_ctx.", "")
        .replace("$props.", "")
        .replace("__props.", "");
    if let Some(render_context) = &ctx.render_context {
        if render_context.as_ref() != "_ctx" {
            cleaned = cleaned.replace(&format!("{render_context}."), "");
        }
    }
    cleaned
}

fn clean_attr_expr(expr: &str, ctx: &VueRecoveryContext) -> String {
    clean_expr(expr, ctx)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn prop_name(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(str) => Some(wtf8_to_string(&str.value)),
        _ => None,
    }
}

fn string_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(str)) => Some(wtf8_to_string(&str.value)),
        _ => None,
    }
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_ascii_lowercase().to_string() + chars.as_str()
}

fn wtf8_to_string(value: &Wtf8Atom) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
