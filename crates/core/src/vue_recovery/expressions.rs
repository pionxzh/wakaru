use anyhow::{anyhow, Result};
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BindingIdent, Decl, Expr, Ident, Module, ModuleItem, Pat, Stmt, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use super::VueRecoveryContext;
use crate::vue_template::{VueExpr, VueNode};

pub(super) fn print_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
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

pub(super) fn clean_expr(expr: &str, ctx: &VueRecoveryContext) -> String {
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

pub(super) fn clean_attr_expr(expr: &str, ctx: &VueRecoveryContext) -> String {
    clean_expr(expr, ctx)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn clean_vue_expr(expr: &str, ctx: &VueRecoveryContext) -> VueExpr {
    VueExpr::new(clean_expr(expr, ctx))
}

pub(super) fn clean_attr_vue_expr(expr: &str, ctx: &VueRecoveryContext) -> VueExpr {
    VueExpr::new(clean_attr_expr(expr, ctx))
}

pub(super) fn printed_vue_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<VueExpr> {
    Ok(clean_attr_vue_expr(&print_expr(expr, ctx)?, ctx))
}

pub(super) fn raw_expr(expr: impl Into<String>) -> VueNode {
    VueNode::RawExpr(VueExpr::new(expr))
}
