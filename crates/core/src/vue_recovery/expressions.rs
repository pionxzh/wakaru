use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignTarget, BindingIdent, BlockStmt, Decl, Expr, Function, Ident,
    IdentName, KeyValueProp, MemberProp, Module, ModuleItem, ObjectPatProp, Pat, Prop, PropName,
    SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::helpers::VueHelper;
use super::VueRecoveryContext;
use crate::rules::UnObjectSpread;
use crate::vue_template::{VueExpr, VueNode, VueUnsupported};

pub(super) fn print_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let mut expr = expr.clone();
    expr.visit_mut_with(&mut ContextMemberCleaner::new(ctx));
    expr.visit_mut_with(&mut SetupAliasCleaner::new(ctx));
    expr.visit_mut_with(&mut SetupRefValueCleaner::new(ctx, true));

    let mut module = Module {
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
    module.visit_mut_with(&mut UnObjectSpread::new());

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

pub(super) fn print_clean_setup_stmt(stmt: &Stmt, ctx: &VueRecoveryContext) -> Result<String> {
    let module = Module {
        span: DUMMY_SP,
        body: vec![ModuleItem::Stmt(stmt.clone())],
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

pub(super) fn clean_setup_stmt(stmt: &Stmt, ctx: &VueRecoveryContext) -> Stmt {
    let mut stmt = stmt.clone();
    stmt.visit_mut_with(&mut ContextMemberCleaner::new(ctx));
    stmt.visit_mut_with(&mut SetupAliasCleaner::new(ctx));
    stmt.visit_mut_with(&mut SetupRefValueCleaner::new(ctx, false));
    stmt
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
    clean_assign_targets: bool,
}

impl<'a> SetupRefValueCleaner<'a> {
    fn new(ctx: &'a VueRecoveryContext, clean_assign_targets: bool) -> Self {
        let bindings = ctx
            .bindings
            .ref_value_cleanup_bindings(clean_assign_targets);
        let shadow_depths = vec![0; bindings.len()];
        Self {
            bindings,
            shadow_depths,
            clean_assign_targets,
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
        let aliases = ctx.bindings.sorted_aliases();
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
    fn visit_mut_prop(&mut self, prop: &mut Prop) {
        if let Prop::Shorthand(ident) = prop {
            if let Some(replacement) = self.active_alias(ident.sym.as_ref()).cloned() {
                *prop = Prop::KeyValue(KeyValueProp {
                    key: PropName::Ident(ident.clone().into()),
                    value: Box::new(Expr::Ident(Ident::new(
                        replacement,
                        DUMMY_SP,
                        Default::default(),
                    ))),
                });
                return;
            }
        }

        prop.visit_mut_children_with(self);
    }

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
        if !self.clean_assign_targets {
            return;
        }

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
            prop_bindings: &ctx.bindings.props,
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

    fn block_shadowing_indices(&self, block: &BlockStmt) -> Vec<usize> {
        let mut indices = block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Decl(decl) => Some(decl),
                _ => None,
            })
            .flat_map(|decl| self.decl_shadowing_indices(decl))
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        indices
    }

    fn decl_shadowing_indices(&self, decl: &Decl) -> Vec<usize> {
        self.prefixes
            .iter()
            .enumerate()
            .filter_map(|(index, prefix)| decl_binds_name(decl, prefix).then_some(index))
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
    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);

        let replacement = match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) if matches!(member.obj.as_ref(), Expr::Ident(object) if self.active_prefix(object.sym.as_ref())) => {
                match &member.prop {
                    MemberProp::Ident(prop) => Some(self.replacement_ident(prop)),
                    MemberProp::Computed(_) | MemberProp::PrivateName(_) => None,
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

    fn visit_mut_block_stmt(&mut self, block: &mut BlockStmt) {
        let shadowed = self.block_shadowing_indices(block);
        self.enter_shadowed(&shadowed);
        block.visit_mut_children_with(self);
        self.exit_shadowed(&shadowed);
    }
}

fn decl_binds_name(decl: &Decl, name: &str) -> bool {
    match decl {
        Decl::Class(class) => class.ident.sym.as_ref() == name,
        Decl::Fn(function) => function.ident.sym.as_ref() == name,
        Decl::Var(var) => var
            .decls
            .iter()
            .any(|decl| pat_binds_name(&decl.name, name)),
        _ => false,
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
    if ctx.bindings.values.is_empty() {
        return input.to_string();
    }

    let mut output = input.to_string();
    for _ in 0..ctx.bindings.values.len() {
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
                if let Some(value) = ctx.bindings.values.iter().find_map(|(binding, value)| {
                    (binding.as_ref() == ident
                        && setup_value_can_inline_in_expr(input, value.value.as_str(), ctx))
                    .then_some(&value.value)
                }) {
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

fn setup_value_can_inline_in_expr(input: &str, value: &str, ctx: &VueRecoveryContext) -> bool {
    let mut refs = HashSet::new();
    super::collect_js_unshadowed_read_refs(value, &mut refs);
    refs.is_empty() || !expr_binds_any_name(input, &refs, ctx)
}

fn expr_binds_any_name(input: &str, names: &HashSet<Atom>, ctx: &VueRecoveryContext) -> bool {
    let Ok(module) =
        super::parse_module(&format!("const __wakaru_expr = {input};"), ctx.cm.clone())
    else {
        return false;
    };
    let mut finder = BindingNameFinder {
        names,
        found: false,
    };
    module.visit_with(&mut finder);
    finder.found
}

struct BindingNameFinder<'a> {
    names: &'a HashSet<Atom>,
    found: bool,
}

impl Visit for BindingNameFinder<'_> {
    fn visit_binding_ident(&mut self, ident: &BindingIdent) {
        if self.names.contains(&ident.id.sym) {
            self.found = true;
        }
    }
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
    if callee.is_empty() {
        return input.to_string();
    }

    let mut output = String::new();
    let mut cursor = 0;

    while let Some(start) = find_callee_call(input, callee, cursor) {
        output.push_str(&input[cursor..start]);
        let open_paren = start + callee.len();
        let Some(close_paren) = matching_paren(input, open_paren) else {
            output.push_str(&input[start..]);
            return output;
        };
        let inner = &input[open_paren + 1..close_paren];
        if should_parenthesize_unwrapped_call(input, start, close_paren, inner) {
            output.push('(');
            output.push_str(inner.trim());
            output.push(')');
        } else {
            output.push_str(inner);
        }
        cursor = close_paren + 1;
    }

    output.push_str(&input[cursor..]);
    output
}

fn find_callee_call(input: &str, callee: &str, from: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut regex = None;

    for (relative, ch) in input[from..].char_indices() {
        let index = from + relative;

        if let Some(state) = regex.as_mut() {
            if regex_is_closed(state, ch) {
                regex = None;
            }
            continue;
        }
        if line_comment {
            if ch == '\n' || ch == '\r' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && input[index + ch.len_utf8()..].starts_with('/') {
                block_comment = false;
            }
            continue;
        }
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
            '"' | '\'' | '`' => {
                quote = Some(ch);
                continue;
            }
            '/' if input[index + ch.len_utf8()..].starts_with('/') => {
                line_comment = true;
                continue;
            }
            '/' if input[index + ch.len_utf8()..].starts_with('*') => {
                block_comment = true;
                continue;
            }
            '/' if slash_starts_regex(input, index) => {
                regex = Some(RegexScanState::default());
                continue;
            }
            _ => {}
        }

        if !input[index..].starts_with(callee) {
            continue;
        }
        let open_paren = index + callee.len();
        if !input[open_paren..].starts_with('(') {
            continue;
        }
        if !is_callee_boundary_before(input, index) {
            continue;
        }
        return Some(index);
    }

    None
}

fn is_callee_boundary_before(input: &str, start: usize) -> bool {
    !input[..start]
        .chars()
        .next_back()
        .is_some_and(|ch| is_ident_continue(ch) || ch == '.')
}

fn should_parenthesize_unwrapped_call(
    input: &str,
    start: usize,
    close_paren: usize,
    inner: &str,
) -> bool {
    if input[..start].trim().is_empty() && input[close_paren + 1..].trim().is_empty() {
        return false;
    }

    let prev = previous_non_ws(input, start);
    let next = next_non_ws(input, close_paren + 1);
    if next.is_some_and(|ch| matches!(ch, '.' | '[' | '(')) && postfix_base_needs_parens(inner) {
        return true;
    }

    if !has_top_level_operator(inner) {
        return false;
    }

    next.is_some_and(|ch| matches!(ch, '.' | '[' | '('))
        || prev.is_some_and(is_expression_operator)
        || next.is_some_and(is_expression_operator)
        || previous_word(input, start).is_some_and(is_prefix_word_operator)
        || previous_word(input, start).is_some_and(is_binary_word_operator)
        || next_word(input, close_paren + 1).is_some_and(is_binary_word_operator)
}

fn postfix_base_needs_parens(input: &str) -> bool {
    let trimmed = input.trim_start();
    trimmed.starts_with('{')
        || starts_with_keyword(trimmed, "function")
        || starts_with_keyword(trimmed, "class")
        || trimmed.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        || has_top_level_operator(input)
}

fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    input
        .strip_prefix(keyword)
        .is_some_and(|rest| rest.chars().next().is_none_or(|ch| !is_ident_continue(ch)))
}

fn previous_non_ws(input: &str, start: usize) -> Option<char> {
    input[..start].chars().rev().find(|ch| !ch.is_whitespace())
}

fn next_non_ws(input: &str, start: usize) -> Option<char> {
    input[start..].chars().find(|ch| !ch.is_whitespace())
}

fn is_expression_operator(ch: char) -> bool {
    matches!(
        ch,
        '!' | '~'
            | '+'
            | '-'
            | '*'
            | '/'
            | '%'
            | '<'
            | '>'
            | '='
            | '&'
            | '|'
            | '^'
            | '?'
            | ':'
            | ','
    )
}

#[derive(Default)]
struct RegexScanState {
    escaped: bool,
    char_class: bool,
}

fn regex_is_closed(state: &mut RegexScanState, ch: char) -> bool {
    if state.escaped {
        state.escaped = false;
        return false;
    }
    match ch {
        '\\' => state.escaped = true,
        '[' => state.char_class = true,
        ']' => state.char_class = false,
        '/' if !state.char_class => return true,
        _ => {}
    }
    false
}

fn slash_starts_regex(input: &str, slash: usize) -> bool {
    let before = input[..slash].trim_end();
    if before.is_empty() {
        return true;
    }
    let Some(prev) = before.chars().next_back() else {
        return true;
    };
    if matches!(
        prev,
        '(' | '['
            | '{'
            | '='
            | ':'
            | ','
            | '!'
            | '?'
            | ';'
            | '+'
            | '-'
            | '*'
            | '/'
            | '%'
            | '&'
            | '|'
            | '^'
            | '~'
            | '<'
            | '>'
    ) {
        return true;
    }
    previous_word(input, slash).is_some_and(|word| {
        matches!(
            word,
            "return"
                | "throw"
                | "case"
                | "delete"
                | "void"
                | "typeof"
                | "in"
                | "instanceof"
                | "new"
                | "yield"
                | "await"
        )
    })
}

fn previous_word(input: &str, end: usize) -> Option<&str> {
    let before = input[..end].trim_end();
    let end = before.len();
    let start = before
        .char_indices()
        .rev()
        .find_map(|(index, ch)| (!is_ident_continue(ch)).then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    (start < end).then_some(&before[start..end])
}

fn next_word(input: &str, start: usize) -> Option<&str> {
    let rest = input[start..].trim_start();
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let end = chars
        .find_map(|(index, ch)| (!is_ident_continue(ch)).then_some(index))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn is_prefix_word_operator(word: &str) -> bool {
    matches!(
        word,
        "typeof" | "void" | "delete" | "await" | "yield" | "new"
    )
}

fn is_binary_word_operator(word: &str) -> bool {
    matches!(word, "in" | "instanceof")
}

fn has_top_level_operator(input: &str) -> bool {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut regex = None;

    for (index, ch) in input.char_indices() {
        if let Some(state) = regex.as_mut() {
            if regex_is_closed(state, ch) {
                regex = None;
            }
            continue;
        }
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
            '/' if slash_starts_regex(input, index) => {
                regex = Some(RegexScanState::default());
            }
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            _ if paren_depth == 0
                && bracket_depth == 0
                && brace_depth == 0
                && is_expression_operator(ch) =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}

fn matching_paren(input: &str, open_paren: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut regex = None;

    for (index, ch) in input[open_paren..].char_indices() {
        let index = open_paren + index;
        if let Some(state) = regex.as_mut() {
            if regex_is_closed(state, ch) {
                regex = None;
            }
            continue;
        }
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
            '/' if slash_starts_regex(input, index) => {
                regex = Some(RegexScanState::default());
            }
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
    let mut regex = None;

    for (index, ch) in input[open_brace..].char_indices() {
        let index = open_brace + index;
        if let Some(state) = regex.as_mut() {
            if regex_is_closed(state, ch) {
                regex = None;
            }
            continue;
        }
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
            '/' if slash_starts_regex(input, index) => {
                regex = Some(RegexScanState::default());
            }
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

pub(super) fn unsupported_vnode_children_expr(expr: impl Into<String>) -> VueNode {
    VueNode::Unsupported(VueUnsupported::vnode_children(VueExpr::new(expr)))
}

#[cfg(test)]
mod tests {
    use super::super::VueRecoveryContext;
    use super::{strip_callee_wrappers, SetupAliasCleaner};
    use swc_core::atoms::Atom;
    use swc_core::common::DUMMY_SP;
    use swc_core::ecma::ast::{Expr, Ident, Prop, PropName};
    use swc_core::ecma::visit::VisitMutWith;

    #[test]
    fn strip_callee_wrappers_requires_identifier_boundary() {
        assert_eq!(strip_callee_wrappers("format(x)", "t"), "format(x)");
    }

    #[test]
    fn strip_callee_wrappers_ignores_string_literals() {
        assert_eq!(
            strip_callee_wrappers(r#"unref(value) + "unref(text)""#, "unref"),
            r#"value + "unref(text)""#
        );
    }

    #[test]
    fn strip_callee_wrappers_preserves_member_precedence() {
        assert_eq!(
            strip_callee_wrappers("unref(a || b).c", "unref"),
            "(a || b).c"
        );
    }

    #[test]
    fn strip_callee_wrappers_parenthesizes_numeric_member_base() {
        assert_eq!(
            strip_callee_wrappers("unref(1).toString()", "unref"),
            "(1).toString()"
        );
    }

    #[test]
    fn strip_callee_wrappers_preserves_call_callee_precedence() {
        assert_eq!(
            strip_callee_wrappers("unref(a || b)(x)", "unref"),
            "(a || b)(x)"
        );
    }

    #[test]
    fn strip_callee_wrappers_preserves_word_operator_precedence() {
        assert_eq!(
            strip_callee_wrappers("typeof unref(a || b)", "unref"),
            "typeof (a || b)"
        );
    }

    #[test]
    fn strip_callee_wrappers_ignores_regex_literals() {
        assert_eq!(
            strip_callee_wrappers("/unref(x)?/.test(value)", "unref"),
            "/unref(x)?/.test(value)"
        );
    }

    #[test]
    fn setup_alias_cleaner_expands_shorthand_property_keys() {
        let mut ctx = VueRecoveryContext::default();
        ctx.bindings
            .aliases
            .insert(Atom::from("p"), Atom::from("props"));
        let mut prop = Prop::Shorthand(Ident::new(Atom::from("p"), DUMMY_SP, Default::default()));

        prop.visit_mut_with(&mut SetupAliasCleaner::new(&ctx));

        let Prop::KeyValue(key_value) = prop else {
            panic!("shorthand property should be expanded when its value is aliased");
        };
        assert!(matches!(&key_value.key, PropName::Ident(key) if key.sym.as_ref() == "p"));
        assert!(
            matches!(key_value.value.as_ref(), Expr::Ident(value) if value.sym.as_ref() == "props")
        );
    }
}
