use anyhow::{anyhow, Result};
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    BindingIdent, Decl, Expr, Ident, Module, ModuleItem, Pat, Stmt, VarDecl, VarDeclKind,
    VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};

use super::helpers::VueHelper;
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
    for (local, helper) in &ctx.vue_helpers {
        if matches!(helper, VueHelper::Unref) {
            cleaned = strip_callee_wrappers(&cleaned, local.as_ref());
        }
    }
    cleaned
}

fn strip_callee_wrappers(input: &str, callee: &str) -> String {
    let pattern = format!("{callee}(");
    let mut output = String::new();
    let mut cursor = 0;

    while let Some(relative_start) = input[cursor..].find(&pattern) {
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);
        let open_paren = start + pattern.len() - 1;
        let Some(close_paren) = matching_paren(input, open_paren) else {
            output.push_str(&input[start..]);
            return output;
        };
        output.push_str(&input[open_paren + 1..close_paren]);
        cursor = close_paren + 1;
    }

    output.push_str(&input[cursor..]);
    output
}

fn matching_paren(input: &str, open_paren: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in input[open_paren..].char_indices() {
        let index = open_paren + index;
        if let Some(current_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == current_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
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
