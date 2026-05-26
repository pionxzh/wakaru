use std::collections::{HashMap, HashSet};

use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmtOrExpr, CallExpr, Callee, Decl, Expr, Lit,
    MemberExpr, MemberProp, Module, ModuleItem, Pat, SimpleAssignTarget, Stmt, TaggedTpl, Tpl,
    TplElement,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::helper_matcher::{
    binding_key, expr_binding_key, remaining_refs_outside_declarations, remove_fn_decls_by_binding,
    remove_var_declarators_by_binding, var_declarator_binding_key, BindingKey,
};

pub struct UnTemplateLiteral;

enum Part {
    Text(String),
    Expr(Box<Expr>),
}

#[derive(Clone)]
struct TemplateData {
    cooked: Vec<Option<String>>,
    raw: Vec<String>,
    helper: Option<BindingKey>,
}

struct TemplateMatch {
    data: TemplateData,
    cache: Option<BindingKey>,
    factory: Option<BindingKey>,
}

impl VisitMut for UnTemplateLiteral {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let factories = collect_template_factories(module);
        let mut replacer = TaggedTemplateReplacer {
            factories: &factories,
            consumed_helpers: HashSet::new(),
            consumed_caches: HashSet::new(),
            consumed_factories: HashSet::new(),
        };
        module.visit_mut_children_with(&mut replacer);
        module.visit_mut_children_with(self);

        let mut removable = replacer.consumed_helpers;
        removable.extend(replacer.consumed_caches);
        removable.extend(replacer.consumed_factories);
        if !removable.is_empty() {
            remove_unused_template_bindings(module, &removable);
        }
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Some(next) = rewrite_concat_chain(expr) {
            *expr = next;
            expr.visit_mut_children_with(self);
            return;
        }
        if let Some(next) = rewrite_plus_chain(expr) {
            *expr = next;
            expr.visit_mut_children_with(self);
            return;
        }

        expr.visit_mut_children_with(self);
    }
}

struct TaggedTemplateReplacer<'a> {
    factories: &'a HashMap<BindingKey, TemplateData>,
    consumed_helpers: HashSet<BindingKey>,
    consumed_caches: HashSet<BindingKey>,
    consumed_factories: HashSet<BindingKey>,
}

impl VisitMut for TaggedTemplateReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(call) = expr else {
            return;
        };
        let Some(next) = rewrite_tagged_template_call(
            call,
            self.factories,
            &mut self.consumed_helpers,
            &mut self.consumed_caches,
            &mut self.consumed_factories,
        ) else {
            return;
        };
        *expr = next;
    }
}

fn rewrite_concat_chain(expr: &Expr) -> Option<Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };

    let mut parts = Vec::new();
    if !collect_concat_parts(call, &mut parts) {
        return None;
    }

    let tpl = parts_to_template(parts, call.span);
    Some(Expr::Tpl(tpl))
}

fn collect_concat_parts(call: &CallExpr, out: &mut Vec<Part>) -> bool {
    let Callee::Expr(callee_expr) = &call.callee else {
        return false;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr else {
        return false;
    };
    if !matches!(prop, MemberProp::Ident(ident) if ident.sym == "concat") {
        return false;
    }

    match &**obj {
        Expr::Call(prev_call) => {
            if !collect_concat_parts(prev_call, out) {
                return false;
            }
        }
        Expr::Lit(Lit::Str(s)) => out.push(Part::Text(s.value.to_string_lossy().into_owned())),
        _ => return false,
    }

    for arg in &call.args {
        if arg.spread.is_some() {
            return false;
        }
        match &*arg.expr {
            Expr::Lit(Lit::Str(s)) => out.push(Part::Text(s.value.to_string_lossy().into_owned())),
            other => out.push(Part::Expr(Box::new(other.clone()))),
        }
    }

    true
}

fn rewrite_tagged_template_call(
    call: &CallExpr,
    factories: &HashMap<BindingKey, TemplateData>,
    consumed_helpers: &mut HashSet<BindingKey>,
    consumed_caches: &mut HashSet<BindingKey>,
    consumed_factories: &mut HashSet<BindingKey>,
) -> Option<Expr> {
    if call.args.is_empty() || call.args.iter().any(|arg| arg.spread.is_some()) {
        return None;
    }

    let template_match = extract_template_match(call.args[0].expr.as_ref(), factories)?;
    if template_match.data.cooked.len() != call.args.len() {
        return None;
    }

    if let Some(helper) = &template_match.data.helper {
        consumed_helpers.insert(helper.clone());
    }
    if let Some(cache) = template_match.cache {
        consumed_caches.insert(cache);
    }
    if let Some(factory) = template_match.factory {
        consumed_factories.insert(factory);
    }

    let Callee::Expr(tag) = &call.callee else {
        return None;
    };
    let exprs = call
        .args
        .iter()
        .skip(1)
        .map(|arg| arg.expr.clone())
        .collect();

    Some(Expr::TaggedTpl(TaggedTpl {
        span: call.span,
        ctxt: call.ctxt,
        tag: tag.clone(),
        type_params: call.type_args.clone(),
        tpl: Box::new(template_match.data.into_tpl(exprs, call.span)),
    }))
}

fn extract_template_match(
    expr: &Expr,
    factories: &HashMap<BindingKey, TemplateData>,
) -> Option<TemplateMatch> {
    let expr = strip_paren_expr(expr);

    if let Some((data, helper)) = extract_direct_template_helper_call(expr) {
        return Some(TemplateMatch {
            data: TemplateData { helper, ..data },
            cache: None,
            factory: None,
        });
    }

    if let Some((factory, data)) = extract_template_factory_call(expr, factories) {
        return Some(TemplateMatch {
            data: data.clone(),
            cache: None,
            factory: Some(factory),
        });
    }

    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };
    let cache = expr_binding_key(strip_paren_expr(left))?;
    let Expr::Assign(assign) = strip_paren_expr(right) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };
    let target_key = binding_key(&target.id);
    if target_key != cache {
        return None;
    }
    let (data, helper) = extract_direct_template_helper_call(strip_paren_expr(&assign.right))?;

    Some(TemplateMatch {
        data: TemplateData { helper, ..data },
        cache: Some(cache),
        factory: None,
    })
}

fn extract_template_factory_call<'a>(
    expr: &Expr,
    factories: &'a HashMap<BindingKey, TemplateData>,
) -> Option<(BindingKey, &'a TemplateData)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if !call.args.is_empty() {
        return None;
    }
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let key = expr_binding_key(strip_paren_expr(callee))?;
    factories
        .get_key_value(&key)
        .map(|(key, data)| (key.clone(), data))
}

fn extract_direct_template_helper_call(expr: &Expr) -> Option<(TemplateData, Option<BindingKey>)> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if call.args.is_empty()
        || call.args.len() > 2
        || call.args.iter().any(|arg| arg.spread.is_some())
    {
        return None;
    }

    let helper = match &call.callee {
        Callee::Expr(callee) => {
            let callee = strip_paren_expr(callee);
            if let Some(helper) = expr_binding_key(callee) {
                if !is_template_helper_name(helper.0.as_ref()) {
                    return None;
                }
                Some(helper)
            } else if is_inline_template_helper(callee) {
                None
            } else {
                return None;
            }
        }
        _ => return None,
    };

    let cooked = collect_template_array(call.args[0].expr.as_ref())?;
    let raw = if call.args.len() == 2 {
        collect_template_array(call.args[1].expr.as_ref())?
            .into_iter()
            .collect::<Option<Vec<_>>>()?
    } else {
        cooked
            .iter()
            .map(|value| value.as_deref().map(escape_template_raw_cooked_copy))
            .collect::<Option<Vec<_>>>()?
    };

    if cooked.len() != raw.len() || cooked.is_empty() {
        return None;
    }

    Some((
        TemplateData {
            cooked,
            raw,
            helper: None,
        },
        helper,
    ))
}

fn is_inline_template_helper(expr: &Expr) -> bool {
    match strip_paren_expr(expr) {
        Expr::Fn(fn_expr) => {
            if fn_expr.function.params.len() != 2 {
                return false;
            }
            if !fn_expr
                .function
                .params
                .iter()
                .all(|param| matches!(param.pat, Pat::Ident(_)))
            {
                return false;
            }
            fn_expr.function.body.as_ref().is_some_and(|body| {
                body.stmts
                    .iter()
                    .any(|stmt| matches!(stmt, Stmt::Return(_)))
            })
        }
        Expr::Arrow(arrow) => {
            arrow.params.len() == 2
                && arrow
                    .params
                    .iter()
                    .all(|param| matches!(param, Pat::Ident(_)))
                && match arrow.body.as_ref() {
                    BlockStmtOrExpr::Expr(_) => true,
                    BlockStmtOrExpr::BlockStmt(body) => body
                        .stmts
                        .iter()
                        .any(|stmt| matches!(stmt, Stmt::Return(_))),
                }
        }
        _ => false,
    }
}

/// Convert `"str" + a + b` (binary `+` chains with at least one string literal)
/// into a template literal.
///
/// Safety: only transforms when at least one operand in the chain is a string
/// literal. All non-string elements that appear **before** the first string
/// literal are grouped into a single sub-expression to preserve arithmetic
/// semantics (e.g. `a + b + "c"` → `` `${a + b}c` `` not `` `${a}${b}c` ``).
fn rewrite_plus_chain(expr: &Expr) -> Option<Expr> {
    // Collect the flat left-associative operand list
    let mut operands: Vec<&Expr> = Vec::new();
    collect_add_chain(expr, &mut operands);

    // Must have at least 2 operands and at least one string literal
    if operands.len() < 2 {
        return None;
    }
    let first_str_idx = operands.iter().position(|e| is_str_lit(e))?;

    // Determine the span for the resulting template
    let span = match expr {
        Expr::Bin(b) => b.span,
        _ => DUMMY_SP,
    };

    // Build the parts list:
    // – everything before the first string literal is grouped into one Expr part
    //   (to avoid splitting arithmetic sub-expressions like `a + b`)
    // – from the first string literal onward, each element becomes Text or Expr
    let mut parts: Vec<Part> = Vec::new();

    if first_str_idx > 0 {
        let grouped = rebuild_add_chain(&operands[..first_str_idx]);
        parts.push(Part::Expr(grouped));
    }

    for op in &operands[first_str_idx..] {
        if let Expr::Lit(Lit::Str(s)) = op {
            parts.push(Part::Text(s.value.to_string_lossy().into_owned()));
        } else {
            parts.push(Part::Expr(Box::new((*op).clone())));
        }
    }

    // Must have at least one Expr part — a pure string-literal chain is not worth
    // converting (it would just be `\`constant\``).
    if !parts.iter().any(|p| matches!(p, Part::Expr(_))) {
        return None;
    }

    Some(Expr::Tpl(parts_to_template(parts, span)))
}

/// Flatten a left-associative `+` chain into individual operands.
fn collect_add_chain<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::Bin(BinExpr {
        op: BinaryOp::Add,
        left,
        right,
        ..
    }) = expr
    {
        collect_add_chain(left, out);
        out.push(right);
    } else {
        out.push(expr);
    }
}

fn is_str_lit(expr: &Expr) -> bool {
    matches!(expr, Expr::Lit(Lit::Str(_)))
}

fn collect_template_factories(module: &Module) -> HashMap<BindingKey, TemplateData> {
    module
        .body
        .iter()
        .filter_map(|item| {
            let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(func))) = item else {
                return None;
            };
            let body = func.function.body.as_ref()?;
            let data = extract_template_from_factory_body(&body.stmts)?;
            Some((binding_key(&func.ident), data))
        })
        .collect()
}

fn extract_template_from_factory_body(stmts: &[Stmt]) -> Option<TemplateData> {
    let mut locals: HashMap<BindingKey, TemplateData> = HashMap::new();

    for stmt in stmts {
        match stmt {
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    let Some(key) = var_declarator_binding_key(decl) else {
                        continue;
                    };
                    let Some(init) = decl.init.as_deref() else {
                        continue;
                    };
                    if let Some((data, helper)) = extract_direct_template_helper_call(init) {
                        locals.insert(key, TemplateData { helper, ..data });
                    }
                }
            }
            Stmt::Return(ret) => {
                let arg = ret.arg.as_deref()?;
                if let Some(key) = expr_binding_key(strip_paren_expr(arg)) {
                    if let Some(data) = locals.get(&key) {
                        return Some(data.clone());
                    }
                }
                if let Some((data, helper)) = extract_direct_template_helper_call(arg) {
                    return Some(TemplateData { helper, ..data });
                }
            }
            _ => {}
        }
    }

    None
}

fn collect_template_array(expr: &Expr) -> Option<Vec<Option<String>>> {
    let Expr::Array(array) = strip_paren_expr(expr) else {
        return None;
    };
    array
        .elems
        .iter()
        .map(|elem| {
            let elem = elem.as_ref()?;
            if elem.spread.is_some() {
                return None;
            }
            match elem.expr.as_ref() {
                Expr::Lit(Lit::Str(s)) => Some(Some(s.value.to_string_lossy().into_owned())),
                Expr::Ident(id) if id.sym.as_ref() == "undefined" => Some(None),
                Expr::Unary(unary)
                    if unary.op == swc_core::ecma::ast::UnaryOp::Void
                        && matches!(unary.arg.as_ref(), Expr::Lit(Lit::Num(_))) =>
                {
                    Some(None)
                }
                _ => None,
            }
        })
        .collect()
}

fn is_template_helper_name(name: &str) -> bool {
    matches!(
        name,
        "_taggedTemplateLiteral"
            | "_taggedTemplateLiteralLoose"
            | "_tagged_template_literal"
            | "__makeTemplateObject"
            | "__template"
    )
}

/// Re-assemble a slice of expressions into a left-associative `+` chain.
fn rebuild_add_chain(exprs: &[&Expr]) -> Box<Expr> {
    debug_assert!(!exprs.is_empty());
    let mut acc = Box::new((*exprs[0]).clone());
    for e in &exprs[1..] {
        acc = Box::new((*acc).make_bin(BinaryOp::Add, (*e).clone()));
    }
    acc
}

impl TemplateData {
    fn into_tpl(self, exprs: Vec<Box<Expr>>, span: swc_core::common::Span) -> Tpl {
        let last = self.cooked.len().saturating_sub(1);
        let quasis = self
            .cooked
            .into_iter()
            .zip(self.raw)
            .enumerate()
            .map(|(index, (cooked, raw))| TplElement {
                span,
                tail: index == last,
                cooked: cooked.map(Into::into),
                raw: raw.into(),
            })
            .collect();

        Tpl {
            span,
            exprs,
            quasis,
        }
    }
}

fn parts_to_template(parts: Vec<Part>, span: swc_core::common::Span) -> Tpl {
    let mut quasis = Vec::new();
    let mut exprs = Vec::new();
    let mut current = String::new();

    for part in parts {
        match part {
            Part::Text(text) => current.push_str(&text),
            Part::Expr(expr) => {
                quasis.push(TplElement {
                    span,
                    tail: false,
                    cooked: Some(current.clone().into()),
                    raw: escape_template_raw(&current).into(),
                });
                current.clear();
                exprs.push(expr);
            }
        }
    }

    quasis.push(TplElement {
        span,
        tail: true,
        cooked: Some(current.clone().into()),
        raw: escape_template_raw(&current).into(),
    });

    Tpl {
        span,
        exprs,
        quasis,
    }
}

fn escape_template_raw(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn escape_template_raw_cooked_copy(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

fn strip_paren_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren_expr(&paren.expr),
        _ => expr,
    }
}

fn remove_unused_template_bindings(module: &mut Module, candidates: &HashSet<BindingKey>) {
    let remaining = remaining_refs_outside_declarations(module, candidates, candidates);
    let unused: HashSet<_> = candidates.difference(&remaining).cloned().collect();
    if unused.is_empty() {
        return;
    }

    remove_fn_decls_by_binding(module, &unused);
    remove_var_declarators_by_binding(&mut module.body, &unused);
}
