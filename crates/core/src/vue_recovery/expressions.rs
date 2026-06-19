use anyhow::{anyhow, Result};
use std::collections::HashMap;
use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BindingIdent, Decl, Expr, Function, Ident, IdentName,
    MemberProp, Module, ModuleItem, ObjectPatProp, Pat, SimpleAssignTarget, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helpers::VueHelper;
use super::VueRecoveryContext;
use crate::vue_template::{VueExpr, VueNode};

pub(super) fn print_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let mut expr = expr.clone();
    expr.visit_mut_with(&mut ContextMemberCleaner::new(ctx));
    expr.visit_mut_with(&mut SetupAliasCleaner::new(ctx));
    expr.visit_mut_with(&mut SetupRefValueCleaner::new(ctx));

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
                init: Some(Box::new(expr)),
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

pub(super) fn print_stmt(stmt: &Stmt, ctx: &VueRecoveryContext) -> Result<String> {
    let mut stmt = stmt.clone();
    stmt.visit_mut_with(&mut ContextMemberCleaner::new(ctx));
    stmt.visit_mut_with(&mut SetupAliasCleaner::new(ctx));
    stmt.visit_mut_with(&mut SetupRefValueCleaner::new(ctx));

    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(stmt)],
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
            .map_err(|error| anyhow!("failed to print Vue setup statement: {error:?}"))?;
    }
    let code = String::from_utf8(output)
        .map(|s| s.trim().to_string())
        .map_err(|error| anyhow!("printed Vue setup statement is not UTF-8: {error}"))?;
    Ok(clean_expr(&code, ctx))
}

pub(super) fn clean_expr(expr: &str, ctx: &VueRecoveryContext) -> String {
    let mut cleaned = expr.to_string();
    for (local, helper) in &ctx.vue_helpers {
        if matches!(helper, VueHelper::Unref) {
            cleaned = strip_callee_wrappers(&cleaned, local.as_ref());
        }
    }
    cleaned = inline_setup_value_bindings(&cleaned, ctx);
    cleaned
}

struct SetupRefValueCleaner<'a> {
    bindings: Vec<&'a str>,
    shadow_depths: Vec<usize>,
}

impl<'a> SetupRefValueCleaner<'a> {
    fn new(ctx: &'a VueRecoveryContext) -> Self {
        let mut bindings = ctx
            .setup_ref_bindings
            .iter()
            .map(|binding| binding.as_ref())
            .collect::<Vec<_>>();
        bindings.sort_unstable();
        bindings.dedup();
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
        }
    }

    fn active_binding(&self, name: &str) -> bool {
        self.bindings
            .iter()
            .zip(self.shadow_depths.iter())
            .any(|(binding, shadow_depth)| *binding == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        self.bindings
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                params
                    .iter()
                    .any(|pat| pat_binds_name(pat, binding))
                    .then_some(index)
            })
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

struct SetupAliasCleaner<'a> {
    aliases: Vec<(&'a str, &'a Atom)>,
    shadow_depths: Vec<usize>,
}

impl<'a> SetupAliasCleaner<'a> {
    fn new(ctx: &'a VueRecoveryContext) -> Self {
        let mut aliases = ctx
            .setup_alias_bindings
            .iter()
            .map(|(from, to)| (from.as_ref(), to))
            .collect::<Vec<_>>();
        aliases.sort_by_key(|(from, _)| *from);
        aliases.dedup_by(|(left, _), (right, _)| left == right);
        let shadow_depths = vec![0; aliases.len()];
        Self {
            aliases,
            shadow_depths,
        }
    }

    fn active_alias(&self, name: &str) -> Option<&Atom> {
        self.aliases
            .iter()
            .zip(self.shadow_depths.iter())
            .find_map(|((from, to), shadow_depth)| {
                (*from == name && *shadow_depth == 0).then_some(*to)
            })
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        self.aliases
            .iter()
            .enumerate()
            .filter_map(|(index, (alias, _))| {
                params
                    .iter()
                    .any(|pat| pat_binds_name(pat, alias))
                    .then_some(index)
            })
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }
}

impl VisitMut for SetupAliasCleaner<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let replacement = match expr {
            Expr::Ident(ident) => self.active_alias(ident.sym.as_ref()).cloned(),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(Ident::new(replacement, DUMMY_SP, Default::default()));
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        function.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }
}

impl VisitMut for SetupRefValueCleaner<'_> {
    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);

        let replacement = match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") => {
                match member.obj.as_ref() {
                    Expr::Ident(object) if self.active_binding(object.sym.as_ref()) => {
                        Some(object.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            assign.left = AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
                id: replacement,
                type_ann: None,
            }));
        }
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let replacement = match expr {
            Expr::Member(member) if matches!(&member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "value") => {
                match member.obj.as_ref() {
                    Expr::Ident(object) if self.active_binding(object.sym.as_ref()) => {
                        Some(object.clone())
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(replacement);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        function.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }
}

struct ContextMemberCleaner<'a> {
    prefixes: Vec<&'a str>,
    prop_bindings: &'a HashMap<Atom, Atom>,
    shadow_depths: Vec<usize>,
}

impl<'a> ContextMemberCleaner<'a> {
    fn new(ctx: &'a VueRecoveryContext) -> Self {
        let mut prefixes = vec!["_ctx", "$props", "__props"];
        if let Some(render_context) = &ctx.render_context {
            if render_context.as_ref() != "_ctx" {
                prefixes.push(render_context.as_ref());
            }
        }
        if let Some(setup_props_context) = &ctx.setup_props_context {
            prefixes.push(setup_props_context.as_ref());
        }
        prefixes.extend(ctx.setup_props_aliases.iter().map(|alias| alias.as_ref()));
        prefixes.sort_unstable();
        prefixes.dedup();
        let shadow_depths = vec![0; prefixes.len()];
        Self {
            prefixes,
            prop_bindings: &ctx.setup_prop_bindings,
            shadow_depths,
        }
    }

    fn active_prefix(&self, name: &str) -> bool {
        self.prefixes
            .iter()
            .zip(self.shadow_depths.iter())
            .any(|(prefix, shadow_depth)| *prefix == name && *shadow_depth == 0)
    }

    fn shadowing_indices(&self, params: &[&Pat]) -> Vec<usize> {
        self.prefixes
            .iter()
            .enumerate()
            .filter_map(|(index, prefix)| {
                params
                    .iter()
                    .any(|pat| pat_binds_name(pat, prefix))
                    .then_some(index)
            })
            .collect()
    }

    fn enter_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] += 1;
        }
    }

    fn exit_shadowed(&mut self, indices: &[usize]) {
        for index in indices {
            self.shadow_depths[*index] -= 1;
        }
    }

    fn replacement_ident(&self, prop: &IdentName) -> Ident {
        let sym = self
            .prop_bindings
            .get(&prop.sym)
            .cloned()
            .unwrap_or_else(|| prop.sym.clone());
        Ident::new(sym, prop.span, Default::default())
    }
}

impl VisitMut for ContextMemberCleaner<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let replacement = match expr {
            Expr::Member(member) if matches!(member.obj.as_ref(), Expr::Ident(object) if self.active_prefix(object.sym.as_ref())) => {
                match &member.prop {
                    MemberProp::Ident(prop) => Some(self.replacement_ident(prop)),
                    MemberProp::Computed(_) | MemberProp::PrivateName(_) => None,
                }
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(replacement);
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        let params = arrow.params.iter().collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        arrow.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }

    fn visit_mut_function(&mut self, function: &mut Function) {
        let params = function
            .params
            .iter()
            .map(|param| &param.pat)
            .collect::<Vec<_>>();
        let shadowed = self.shadowing_indices(&params);
        self.enter_shadowed(&shadowed);
        function.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }
}

fn pat_binds_name(pat: &Pat, name: &str) -> bool {
    match pat {
        Pat::Ident(binding) => binding.id.sym.as_ref() == name,
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| pat_binds_name(elem, name)),
        Pat::Rest(rest) => pat_binds_name(rest.arg.as_ref(), name),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(key_value) => pat_binds_name(key_value.value.as_ref(), name),
            ObjectPatProp::Assign(assign) => assign.key.sym.as_ref() == name,
            ObjectPatProp::Rest(rest) => pat_binds_name(rest.arg.as_ref(), name),
        }),
        Pat::Assign(assign) => pat_binds_name(assign.left.as_ref(), name),
        Pat::Expr(_) | Pat::Invalid(_) => false,
    }
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

        if matches!(ch, '"' | '\'') {
            quote = Some(ch);
            output.push(ch);
            cursor += ch_len;
            continue;
        }

        if ch == '`' {
            let (template, template_changed, next_cursor) =
                replace_template_literal_bindings_once(input, cursor, ctx);
            output.push_str(&template);
            cursor = next_cursor;
            changed |= template_changed;
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

fn replace_template_literal_bindings_once(
    input: &str,
    start: usize,
    ctx: &VueRecoveryContext,
) -> (String, bool, usize) {
    let mut output = String::new();
    let mut cursor = start;
    let mut changed = false;
    let mut escaped = false;

    while cursor < input.len() {
        let Some(ch) = input[cursor..].chars().next() else {
            break;
        };
        let ch_len = ch.len_utf8();
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
        if ch == '`' && cursor > start + ch_len {
            break;
        }
        if ch == '$' && input[cursor..].starts_with('{') {
            output.push('{');
            let open_brace = cursor;
            let Some(close_brace) = matching_brace(input, open_brace) else {
                output.push_str(&input[cursor + 1..]);
                return (output, changed, input.len());
            };
            let (inner, inner_changed) =
                replace_setup_value_bindings_once(&input[open_brace + 1..close_brace], ctx);
            output.push_str(&inner);
            output.push('}');
            cursor = close_brace + 1;
            changed |= inner_changed;
        }
    }

    (output, changed, cursor)
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

fn matching_brace(input: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for (index, ch) in input[open_brace..].char_indices() {
        let index = open_brace + index;
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
            '{' => depth += 1,
            '}' => {
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
