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
    if let Some(setup_props_context) = &ctx.setup_props_context {
        cleaned = cleaned.replace(&format!("{setup_props_context}."), "");
    }
    for setup_props_alias in &ctx.setup_props_aliases {
        cleaned = cleaned.replace(&format!("{setup_props_alias}."), "");
    }
    for (local, helper) in &ctx.vue_helpers {
        if matches!(helper, VueHelper::Unref) {
            cleaned = strip_callee_wrappers(&cleaned, local.as_ref());
        }
    }
    cleaned = inline_setup_value_bindings(&cleaned, ctx);
    cleaned
}

fn inline_setup_value_bindings(input: &str, ctx: &VueRecoveryContext) -> String {
    if ctx.setup_value_bindings.is_empty() {
        return input.to_string();
    }

    let mut output = input.to_string();
    for _ in 0..ctx.setup_value_bindings.len() {
        let (next, changed) = replace_setup_value_bindings_once(&output, ctx);
        output = next;
        if !changed {
            break;
        }
    }

    strip_outer_parens(&output)
}

fn replace_setup_value_bindings_once(input: &str, ctx: &VueRecoveryContext) -> (String, bool) {
    let mut output = String::new();
    let mut cursor = 0;
    let mut changed = false;
    let mut quote = None;
    let mut escaped = false;

    while cursor < input.len() {
        let Some(ch) = input[cursor..].chars().next() else {
            break;
        };
        let ch_len = ch.len_utf8();

        if let Some(current_quote) = quote {
            output.push(ch);
            cursor += ch_len;
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

        if matches!(ch, '"' | '\'' | '`') {
            quote = Some(ch);
            output.push(ch);
            cursor += ch_len;
            continue;
        }

        if is_ident_start(ch) && is_reference_start(input, cursor) {
            let start = cursor;
            cursor += ch_len;
            while cursor < input.len() {
                let Some(next) = input[cursor..].chars().next() else {
                    break;
                };
                if !is_ident_continue(next) {
                    break;
                }
                cursor += next.len_utf8();
            }

            let ident = &input[start..cursor];
            if input[cursor..].starts_with(".value") {
                if let Some(value) = ctx
                    .setup_value_bindings
                    .iter()
                    .find_map(|(binding, value)| (binding.as_ref() == ident).then_some(value))
                {
                    output.push_str(&format!("({})", value.trim()));
                    cursor += ".value".len();
                    changed = true;
                    continue;
                }
            }

            output.push_str(&input[start..cursor]);
            continue;
        }

        output.push(ch);
        cursor += ch_len;
    }

    (output, changed)
}

fn is_reference_start(input: &str, cursor: usize) -> bool {
    !input[..cursor]
        .chars()
        .next_back()
        .is_some_and(|ch| is_ident_continue(ch) || ch == '.')
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

fn strip_outer_parens(input: &str) -> String {
    let mut trimmed = input.trim();
    while trimmed.starts_with('(')
        && matching_paren(trimmed, 0).is_some_and(|close| close == trimmed.len() - 1)
    {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }
    trimmed.to_string()
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
