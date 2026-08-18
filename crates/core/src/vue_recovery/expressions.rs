use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use swc_core::atoms::Atom;
use swc_core::common::{Globals, Mark, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_core::ecma::ast::{
    AssignExpr, AssignTarget, BindingIdent, Decl, Expr, ExprStmt, Ident, MemberProp, Module,
    ModuleItem, Pat, SimpleAssignTarget, Stmt, VarDecl, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::codegen::{text_writer::JsWriter, Config, Emitter};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::helpers::VueHelper;
use super::VueRecoveryContext;
use crate::rules::rename_utils::rename_bindings;
use crate::rules::UnObjectSpread;
use crate::vue_template::{VueExpr, VueNode, VueUnsupported};

pub(super) fn print_expr(expr: &Expr, ctx: &VueRecoveryContext) -> Result<String> {
    let mut expr = expr.clone();
    clean_context_members_in_expr(&mut expr, ctx);
    rename_bindings(&mut expr, &super::setup_alias_renames(ctx));
    let setup_refs = unresolved_expr_ident_ptrs(&expr);
    expr.visit_mut_with(&mut SetupRefValueCleaner::new(ctx, setup_refs, true));

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
    clean_setup_stmt_with_ref_values(stmt, ctx, false)
}

pub(super) fn clean_setup_stmt_preserving_ref_values(
    stmt: &Stmt,
    ctx: &VueRecoveryContext,
) -> Stmt {
    clean_setup_stmt_with_ref_values(stmt, ctx, true)
}

fn clean_setup_stmt_with_ref_values(
    stmt: &Stmt,
    ctx: &VueRecoveryContext,
    preserve_ref_values: bool,
) -> Stmt {
    let mut stmt = stmt.clone();
    clean_context_members_in_stmt(&mut stmt, ctx);
    rename_bindings(&mut stmt, &super::setup_alias_renames(ctx));
    if !preserve_ref_values {
        let setup_refs = unresolved_stmt_ident_ptrs(&stmt);
        stmt.visit_mut_with(&mut SetupRefValueCleaner::new(ctx, setup_refs, false));
    }
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

/// Classifies references in a detached expression or statement with SWC's
/// resolver while preserving the syntax contexts used by the original module.
///
/// Vue recovery mixes nodes cloned from the resolved input module with nodes
/// parsed from generated template fragments. Their original syntax contexts
/// are therefore not comparable. A context-free probe is re-resolved as strict
/// module code, then its unresolved identifiers are paired with the matching
/// nodes in the real tree. The cleaners can leave every local shadow alone
/// without changing binding identities used by later analysis.
struct ClearSyntaxContexts;

impl VisitMut for ClearSyntaxContexts {
    fn visit_mut_syntax_context(&mut self, ctxt: &mut SyntaxContext) {
        *ctxt = SyntaxContext::empty();
    }
}

fn resolve_module_for_cleaning(module: &mut Module) -> SyntaxContext {
    module.visit_mut_with(&mut ClearSyntaxContexts);
    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    module.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
    SyntaxContext::empty().apply_mark(unresolved_mark)
}

fn with_resolver_globals<T>(operation: impl FnOnce() -> T) -> T {
    if GLOBALS.is_set() {
        operation()
    } else {
        let globals = Globals::new();
        GLOBALS.set(&globals, operation)
    }
}

struct UnresolvedIdentCollector {
    unresolved_ctxt: SyntaxContext,
    classifications: Vec<(Atom, bool)>,
}

impl Visit for UnresolvedIdentCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        self.classifications
            .push((ident.sym.clone(), ident.ctxt == self.unresolved_ctxt));
    }
}

fn unresolved_expr_ident_classifications(expr: &Expr) -> Vec<(Atom, bool)> {
    with_resolver_globals(|| {
        let mut module = Module {
            span: DUMMY_SP,
            body: vec![ModuleItem::Stmt(Stmt::Expr(ExprStmt {
                span: DUMMY_SP,
                expr: Box::new(expr.clone()),
            }))],
            shebang: None,
        };
        let unresolved_ctxt = resolve_module_for_cleaning(&mut module);
        let mut collector = UnresolvedIdentCollector {
            unresolved_ctxt,
            classifications: Vec::new(),
        };
        module.visit_with(&mut collector);
        collector.classifications
    })
}

fn unresolved_stmt_ident_classifications(stmt: &Stmt) -> Vec<(Atom, bool)> {
    with_resolver_globals(|| {
        let mut module = Module {
            span: DUMMY_SP,
            body: vec![ModuleItem::Stmt(stmt.clone())],
            shebang: None,
        };
        let unresolved_ctxt = resolve_module_for_cleaning(&mut module);
        let mut collector = UnresolvedIdentCollector {
            unresolved_ctxt,
            classifications: Vec::new(),
        };
        module.visit_with(&mut collector);
        collector.classifications
    })
}

struct UnresolvedIdentPtrCollector<'a> {
    classifications: std::slice::Iter<'a, (Atom, bool)>,
    unresolved: HashSet<*const Ident>,
}

impl Visit for UnresolvedIdentPtrCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let (probe_sym, unresolved) = self
            .classifications
            .next()
            .expect("resolver probe should have the same identifiers as its source");
        assert_eq!(
            probe_sym, &ident.sym,
            "resolver probe should preserve identifier traversal order"
        );
        if *unresolved {
            self.unresolved.insert(std::ptr::from_ref(ident));
        }
    }
}

fn unresolved_ident_ptrs<'a>(
    node: &impl VisitWith<UnresolvedIdentPtrCollector<'a>>,
    classifications: &'a [(Atom, bool)],
) -> HashSet<*const Ident> {
    let mut collector = UnresolvedIdentPtrCollector {
        classifications: classifications.iter(),
        unresolved: HashSet::new(),
    };
    node.visit_with(&mut collector);
    assert!(
        collector.classifications.next().is_none(),
        "resolver probe should have the same identifiers as its source"
    );
    collector.unresolved
}

fn unresolved_expr_ident_ptrs(expr: &Expr) -> HashSet<*const Ident> {
    let classifications = unresolved_expr_ident_classifications(expr);
    unresolved_ident_ptrs(expr, &classifications)
}

fn unresolved_stmt_ident_ptrs(stmt: &Stmt) -> HashSet<*const Ident> {
    let classifications = unresolved_stmt_ident_classifications(stmt);
    unresolved_ident_ptrs(stmt, &classifications)
}

struct SetupRefValueCleaner<'a> {
    bindings: Vec<&'a str>,
    unresolved: HashSet<*const Ident>,
    clean_assign_targets: bool,
}

impl<'a> SetupRefValueCleaner<'a> {
    fn new(
        ctx: &'a VueRecoveryContext,
        unresolved: HashSet<*const Ident>,
        clean_assign_targets: bool,
    ) -> Self {
        let bindings = ctx
            .bindings
            .ref_value_cleanup_bindings(clean_assign_targets);
        Self {
            bindings,
            unresolved,
            clean_assign_targets,
        }
    }

    fn active_binding(&self, ident: &Ident) -> bool {
        self.unresolved.contains(&std::ptr::from_ref(ident))
            && self
                .bindings
                .iter()
                .any(|binding| *binding == ident.sym.as_ref())
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
                    Expr::Ident(object) if self.active_binding(object) => Some(object.clone()),
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
                    Expr::Ident(object) if self.active_binding(object) => Some(object.clone()),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(replacement);
        }
    }
}

struct ContextMemberCleaner<'a> {
    prefixes: Vec<&'a str>,
    prop_bindings: &'a HashMap<Atom, Atom>,
    output_unresolved_ctxt: SyntaxContext,
    unresolved: HashSet<*const Ident>,
    needs_another_pass: bool,
}

impl<'a> ContextMemberCleaner<'a> {
    fn new(ctx: &'a VueRecoveryContext, unresolved: HashSet<*const Ident>) -> Self {
        let mut prefixes = vec!["_ctx", "$props", "__props"];
        if let Some(render_context) = &ctx.render_context {
            if render_context.as_ref() != "_ctx" {
                prefixes.push(render_context.as_ref());
            }
        }
        if let Some(render_props_context) = &ctx.render_props_context {
            prefixes.push(render_props_context.as_ref());
        }
        if let Some(render_setup_context) = &ctx.render_setup_context {
            prefixes.push(render_setup_context.as_ref());
        }
        if let Some(setup_props_context) = &ctx.setup_props_context {
            prefixes.push(setup_props_context.as_ref());
        }
        prefixes.extend(ctx.setup_props_aliases.iter().map(|alias| alias.as_ref()));
        prefixes.sort_unstable();
        prefixes.dedup();
        Self {
            prefixes,
            prop_bindings: &ctx.bindings.props,
            output_unresolved_ctxt: ctx.unresolved_ctxt,
            unresolved,
            needs_another_pass: false,
        }
    }

    fn active_prefix(&self, ident: &Ident) -> bool {
        self.unresolved.contains(&std::ptr::from_ref(ident))
            && self
                .prefixes
                .iter()
                .any(|prefix| *prefix == ident.sym.as_ref())
    }

    fn replacement_ident(&mut self, prop: &MemberProp) -> Option<Ident> {
        let (sym, span) = match prop {
            MemberProp::Ident(prop) => (prop.sym.clone(), prop.span),
            MemberProp::Computed(computed) => {
                let Expr::Lit(swc_core::ecma::ast::Lit::Str(value)) = computed.expr.as_ref() else {
                    return None;
                };
                let name = super::syntax::wtf8_to_string(&value.value);
                if !crate::js_names::is_valid_identifier_name(&name) {
                    return None;
                }
                (Atom::from(name), computed.span)
            }
            MemberProp::PrivateName(_) => return None,
        };
        let sym = self.prop_bindings.get(&sym).cloned().unwrap_or(sym);
        self.needs_another_pass |= self.prefixes.iter().any(|prefix| *prefix == sym.as_ref());
        // The collapsed member access (`_ctx.foo` -> `foo`) is a free reference
        // to a template-scope binding. The next resolver probe classifies it for
        // ref cleanup while this context preserves later binding analysis.
        Some(Ident::new(sym, span, self.output_unresolved_ctxt))
    }
}

impl VisitMut for ContextMemberCleaner<'_> {
    fn visit_mut_assign_expr(&mut self, assign: &mut AssignExpr) {
        assign.visit_mut_children_with(self);

        let replacement = match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) if matches!(member.obj.as_ref(), Expr::Ident(object) if self.active_prefix(object)) => {
                self.replacement_ident(&member.prop)
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
            Expr::Member(member) if matches!(member.obj.as_ref(), Expr::Ident(object) if self.active_prefix(object)) => {
                self.replacement_ident(&member.prop)
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = Expr::Ident(replacement);
        }
    }
}

fn clean_context_members_in_expr(expr: &mut Expr, ctx: &VueRecoveryContext) {
    loop {
        let unresolved = unresolved_expr_ident_ptrs(expr);
        let mut cleaner = ContextMemberCleaner::new(ctx, unresolved);
        expr.visit_mut_with(&mut cleaner);
        if !cleaner.needs_another_pass {
            break;
        }
    }
}

fn clean_context_members_in_stmt(stmt: &mut Stmt, ctx: &VueRecoveryContext) {
    loop {
        let unresolved = unresolved_stmt_ident_ptrs(stmt);
        let mut cleaner = ContextMemberCleaner::new(ctx, unresolved);
        stmt.visit_mut_with(&mut cleaner);
        if !cleaner.needs_another_pass {
            break;
        }
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
    if next.is_some_and(|ch| matches!(ch, '.' | '[' | '(' | '`'))
        && postfix_base_needs_parens(inner)
    {
        return true;
    }

    if !has_top_level_operator(inner) {
        return false;
    }

    next.is_some_and(|ch| matches!(ch, '.' | '[' | '(' | '`'))
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
    use super::strip_callee_wrappers;

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
    fn strip_callee_wrappers_preserves_tagged_template_precedence() {
        assert_eq!(
            strip_callee_wrappers("unref(a || b)`x`", "unref"),
            "(a || b)`x`"
        );
        assert_eq!(strip_callee_wrappers("unref(tag)`x`", "unref"), "tag`x`");
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
}
