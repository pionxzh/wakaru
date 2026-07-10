use swc_core::atoms::Atom;
use swc_core::common::{Mark, Spanned, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayPat, ArrowExpr, AssignExpr, AssignOp, AssignPat, AssignPatProp, AssignTarget, BinExpr,
    BinaryOp, BindingIdent, BlockStmt, BlockStmtOrExpr, Bool, CatchClause, ClassDecl,
    ComputedPropName, Decl, Expr, FnDecl, Function, Ident, IdentName, IfStmt, KeyValuePatProp, Lit,
    MemberExpr, MemberProp, Number, ObjectPat, ObjectPatProp, Param, Pat, Prop, PropName,
    SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp, UpdateExpr, VarDecl, VarDeclKind,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::js_names::{is_likely_generated_alias, is_reserved_binding_name};

use super::decl_utils::same_ident;
use super::rename_utils::{rename_bindings, BindingRename};
use super::RewriteLevel;
use crate::utils::paren::strip_parens;

type BindingId = (Atom, SyntaxContext);

pub struct UnParameters {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnParameters {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

impl VisitMut for UnParameters {
    fn visit_mut_function(&mut self, func: &mut Function) {
        func.visit_mut_children_with(self);
        if let Some(body) = &mut func.body {
            process_function_params(&mut func.params, body, self.level, self.unresolved_mark);
        }
    }

    fn visit_mut_arrow_expr(&mut self, expr: &mut ArrowExpr) {
        expr.visit_mut_children_with(self);
        if let BlockStmtOrExpr::BlockStmt(body) = &mut *expr.body {
            process_arrow_params(&mut expr.params, body, self.level, self.unresolved_mark);
        }
    }
}

/// Process Pattern A (TypeScript/Babel simple form) and Pattern B (arguments-based)
/// for regular functions with Vec<Param>.
fn process_function_params(
    params: &mut Vec<Param>,
    body: &mut BlockStmt,
    level: RewriteLevel,
    unresolved_mark: Mark,
) {
    let body_bindings = collect_body_bindings(body);

    process_pattern_a_params(params, body, unresolved_mark, &body_bindings);
    if level >= RewriteLevel::Standard {
        process_pattern_c_params(params, body, unresolved_mark, &body_bindings);
        process_pattern_b_params(params, body, unresolved_mark, &body_bindings);
        rewrite_inline_arguments_defaults(params, body, unresolved_mark, &body_bindings);
        materialize_inline_temp_defaults(body, unresolved_mark);
        fold_object_property_param_aliases(params, body, unresolved_mark);
        fold_array_index_param_aliases(params, body, unresolved_mark);
        fold_destructured_param_aliases(params, body, unresolved_mark);
        if promote_destructured_binding_defaults(params, body, unresolved_mark) {
            fold_object_property_param_aliases(params, body, unresolved_mark);
            fold_array_index_param_aliases(params, body, unresolved_mark);
        }
    }
}

/// Process Pattern A for arrow functions with Vec<Pat>.
fn process_arrow_params(
    params: &mut Vec<Pat>,
    body: &mut BlockStmt,
    level: RewriteLevel,
    unresolved_mark: Mark,
) {
    let body_bindings = collect_body_bindings(body);

    process_pattern_a_arrow_params(params, body, unresolved_mark, &body_bindings);
    if level >= RewriteLevel::Standard {
        process_pattern_c_arrow_params(params, body, unresolved_mark, &body_bindings);
        fold_destructured_arrow_param_aliases(params, body, unresolved_mark);
    }
}

// ============================================================
// Pattern A: `if (a === void 0) { a = 1; }` → default param
// ============================================================

fn process_pattern_a_params(
    params: &mut Vec<Param>,
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let mut to_remove: Vec<usize> = Vec::new();

    // Only scan first 15 statements
    let scan_limit = body.stmts.len().min(15);

    for (stmt_idx, stmt) in body.stmts[..scan_limit].iter().enumerate() {
        let extracted = extract_default_param_from_if(stmt, unresolved_mark);

        if let Some((param_ident, default_val)) = extracted {
            // Find the matching parameter
            if let Some(param_idx) = find_plain_param_idx(params, &param_ident) {
                if default_references_blocked_param_binding(
                    params,
                    param_idx,
                    &default_val,
                    body_bindings,
                ) {
                    continue;
                }
                // Replace the param with an assignment pattern
                let original_pat =
                    std::mem::replace(&mut params[param_idx].pat, Pat::Invalid(Default::default()));
                params[param_idx].pat = Pat::Assign(AssignPat {
                    span: DUMMY_SP,
                    left: Box::new(original_pat),
                    right: default_val,
                });
                to_remove.push(stmt_idx);
            }
        }
    }

    // Remove matched if statements (in reverse order to preserve indices)
    for idx in to_remove.into_iter().rev() {
        body.stmts.remove(idx);
    }
}

fn process_pattern_a_arrow_params(
    params: &mut Vec<Pat>,
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let mut to_remove: Vec<usize> = Vec::new();

    let scan_limit = body.stmts.len().min(15);

    for (stmt_idx, stmt) in body.stmts[..scan_limit].iter().enumerate() {
        let extracted = extract_default_param_from_if(stmt, unresolved_mark);

        if let Some((param_ident, default_val)) = extracted {
            if let Some(param_idx) = find_plain_pat_idx(params, &param_ident) {
                if default_references_blocked_arrow_binding(
                    params,
                    param_idx,
                    &default_val,
                    body_bindings,
                ) {
                    continue;
                }
                let original_pat =
                    std::mem::replace(&mut params[param_idx], Pat::Invalid(Default::default()));
                params[param_idx] = Pat::Assign(AssignPat {
                    span: DUMMY_SP,
                    left: Box::new(original_pat),
                    right: default_val,
                });
                to_remove.push(stmt_idx);
            }
        }
    }

    for idx in to_remove.into_iter().rev() {
        body.stmts.remove(idx);
    }
}

/// Extract `(param_name, default_value)` from:
/// - `if (a === void 0) a = 1;`
/// - `if (a === void 0) { a = 1; }`
/// - `if (void 0 === a) a = 1;`  (also handles `undefined`)
fn extract_default_param_from_if(stmt: &Stmt, unresolved_mark: Mark) -> Option<(Ident, Box<Expr>)> {
    let Stmt::If(IfStmt {
        test, cons, alt, ..
    }) = stmt
    else {
        return None;
    };
    if alt.is_some() {
        return None;
    }

    let param_ident = extract_void0_check(test, unresolved_mark)?;
    let default_val = extract_assign_from_cons(cons, &param_ident)?;

    Some((param_ident, default_val))
}

/// Check if `expr` is `ident === void 0` or `void 0 === ident`
/// or `ident === undefined` or `undefined === ident`.
/// Returns the checked identifier if matched.
fn extract_void0_check(expr: &Expr, unresolved_mark: Mark) -> Option<Ident> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if *op != BinaryOp::EqEqEq {
        return None;
    }

    // left === void 0 / left === undefined
    if is_void0_or_undefined(right, unresolved_mark) {
        if let Expr::Ident(id) = left.as_ref() {
            return Some(id.clone());
        }
    }
    // void 0 === right / undefined === right
    if is_void0_or_undefined(left, unresolved_mark) {
        if let Expr::Ident(id) = right.as_ref() {
            return Some(id.clone());
        }
    }

    None
}

fn is_void0_or_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    // void 0 (or void <num>)
    if let Expr::Unary(UnaryExpr {
        op: UnaryOp::Void,
        arg,
        ..
    }) = expr
    {
        if matches!(arg.as_ref(), Expr::Lit(_)) {
            return true;
        }
    }
    // undefined identifier
    if let Expr::Ident(id) = expr {
        if is_undefined_ident(id, unresolved_mark) {
            return true;
        }
    }
    false
}

fn is_undefined_ident(id: &Ident, unresolved_mark: Mark) -> bool {
    id.sym.as_ref() == "undefined"
        && (id.ctxt.outer() == unresolved_mark || id.ctxt == SyntaxContext::empty())
}

fn is_arguments_ident(id: &Ident, unresolved_mark: Mark) -> bool {
    id.sym.as_ref() == "arguments" && id.ctxt.outer() == unresolved_mark
}

/// Extract the assigned default value from the consequent branch.
/// Consequent can be:
/// - `ExprStmt(Assign { left: ident, op: =, right: val })`
/// - `Block([ExprStmt(Assign { left: ident, op: =, right: val })])`
fn extract_assign_from_cons(cons: &Stmt, param_ident: &Ident) -> Option<Box<Expr>> {
    match cons {
        Stmt::Expr(expr_stmt) => extract_assign_expr(&expr_stmt.expr, param_ident),
        Stmt::Block(block) => {
            if block.stmts.len() != 1 {
                return None;
            }
            let Stmt::Expr(expr_stmt) = &block.stmts[0] else {
                return None;
            };
            extract_assign_expr(&expr_stmt.expr, param_ident)
        }
        _ => None,
    }
}

fn extract_assign_expr(expr: &Expr, param_ident: &Ident) -> Option<Box<Expr>> {
    let Expr::Assign(AssignExpr {
        op: AssignOp::Assign,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Ident(ident)) = left else {
        return None;
    };
    if !same_ident(&ident.id, param_ident) {
        return None;
    }
    Some(right.clone())
}

// ============================================================
// Pattern C: `const alias = param === undefined ? {} : param`
// ============================================================

fn process_pattern_c_params(
    params: &mut [Param],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let mut stmt_idx = 0;

    while stmt_idx < body.stmts.len().min(15) {
        let Some((param_ident, default_val)) =
            extract_param_object_default_stmt(&body.stmts[stmt_idx], unresolved_mark)
        else {
            break;
        };
        let Some((param_idx, param_ident)) = find_plain_param_ident(params, &param_ident) else {
            break;
        };
        if !set_param_default(params, param_idx, default_val, body_bindings) {
            break;
        }
        rewrite_param_object_default_stmt(&mut body.stmts[stmt_idx], param_ident.clone());
        if !try_inline_dead_alias(&mut body.stmts, stmt_idx, &param_ident, unresolved_mark) {
            stmt_idx += 1;
        }
    }
}

/// After Pattern C rewrites `let alias = param === void 0 ? {} : param` to
/// `let alias = param`, try to inline the now-dead alias so that downstream
/// property folding and reassignment-default promotion can reach through to
/// the param. Only fires when a property of the alias has a reassignment
/// default in the body (the Babel 7.8 nested destructuring pattern) — this
/// avoids regressing cases where the fold's emitted-name safety check would
/// bail out on the param name shadowed in a nested scope. Returns true if
/// the alias declaration was removed.
fn try_inline_dead_alias(
    stmts: &mut Vec<Stmt>,
    alias_idx: usize,
    param_ident: &Ident,
    unresolved_mark: Mark,
) -> bool {
    let Stmt::Decl(Decl::Var(var)) = &stmts[alias_idx] else {
        return false;
    };
    if var.decls.len() != 1 {
        return false;
    }
    let Pat::Ident(alias_binding) = &var.decls[0].name else {
        return false;
    };
    let alias = &alias_binding.id;
    if same_ident(alias, param_ident) {
        return false;
    }
    if !is_likely_generated_alias(&alias.sym) {
        return false;
    }

    let rest = &stmts[alias_idx + 1..];

    // Only inline when a property of the alias has a recognized default in the
    // following statement. Without this, the inline just shifts naming work to
    // SmartInline and can regress property-fold naming via the emitted-name
    // safety check.
    if !has_property_default(rest, alias, unresolved_mark) {
        return false;
    }

    if ident_is_assigned_in(rest, param_ident) {
        return false;
    }

    let rename = [BindingRename {
        old: (alias.sym.clone(), alias.ctxt),
        new: param_ident.sym.clone(),
    }];
    for stmt in &mut stmts[alias_idx + 1..] {
        rename_bindings(stmt, &rename);
    }
    stmts.remove(alias_idx);
    true
}

/// True when `stmts` contains a property access on `alias` whose local binding
/// receives a void-0 default in the following assignment or declaration.
fn has_property_default(stmts: &[Stmt], alias: &Ident, unresolved_mark: Mark) -> bool {
    for (i, stmt) in stmts.iter().enumerate() {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            continue;
        };
        if var.decls.len() != 1 {
            continue;
        }
        let decl = &var.decls[0];
        let Pat::Ident(local) = &decl.name else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        let Expr::Member(member) = strip_parens(init.as_ref()) else {
            continue;
        };
        let Expr::Ident(obj) = member.obj.as_ref() else {
            continue;
        };
        if !same_ident(obj, alias) {
            continue;
        }
        if let Some(next) = stmts.get(i + 1) {
            let has_computed_declaration_default =
                matches!(
                    &member.prop,
                    MemberProp::Computed(computed)
                        if matches!(strip_parens(&computed.expr), Expr::Ident(_))
                ) && extract_default_from_temp_stmt(next, &local.id, unresolved_mark).is_some();
            if is_reassignment_default_for(next, &local.id) || has_computed_declaration_default {
                return true;
            }
        }
    }
    false
}

fn is_reassignment_default_for(stmt: &Stmt, binding: &Ident) -> bool {
    let Stmt::Expr(expr_stmt) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return false;
    };
    same_ident(&target.id, binding)
}

fn ident_is_assigned_in(stmts: &[Stmt], ident: &Ident) -> bool {
    let mut finder = AssignFinder {
        target: ident,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

struct AssignFinder<'a> {
    target: &'a Ident,
    found: bool,
}

impl Visit for AssignFinder<'_> {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left {
            if same_ident(&binding.id, self.target) {
                self.found = true;
                return;
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(id) = update.arg.as_ref() {
            if same_ident(id, self.target) {
                self.found = true;
                return;
            }
        }
        update.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn process_pattern_c_arrow_params(
    params: &mut [Pat],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let mut stmt_idx = 0;

    while stmt_idx < body.stmts.len().min(15) {
        let Some((param_ident, default_val)) =
            extract_param_object_default_stmt(&body.stmts[stmt_idx], unresolved_mark)
        else {
            break;
        };
        let Some((param_idx, param_ident)) = find_plain_arrow_param_ident(params, &param_ident)
        else {
            break;
        };
        if !set_arrow_param_default(params, param_idx, default_val, body_bindings) {
            break;
        }
        rewrite_param_object_default_stmt(&mut body.stmts[stmt_idx], param_ident.clone());
        if !try_inline_dead_alias(&mut body.stmts, stmt_idx, &param_ident, unresolved_mark) {
            stmt_idx += 1;
        }
    }
}

fn extract_param_object_default_stmt(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(Ident, Box<Expr>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let init = var.decls[0].init.as_ref()?;
    extract_param_object_default_expr(init, unresolved_mark)
}

fn extract_param_object_default_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<(Ident, Box<Expr>)> {
    let Expr::Cond(cond) = strip_parens(expr) else {
        return None;
    };
    let param_ident = extract_void0_check(cond.test.as_ref(), unresolved_mark)?;
    if !is_empty_object_literal(cond.cons.as_ref()) {
        return None;
    }
    if !is_ident_expr(cond.alt.as_ref(), &param_ident) {
        return None;
    }
    Some((param_ident, cond.cons.clone()))
}

fn rewrite_param_object_default_stmt(stmt: &mut Stmt, param_ident: Ident) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    if let Some(decl) = var.decls.first_mut() {
        decl.init = Some(Box::new(Expr::Ident(param_ident)));
    }
}

fn find_plain_param_ident(params: &[Param], ident: &Ident) -> Option<(usize, Ident)> {
    if !plain_param_name_is_unique(params, ident) {
        return None;
    }
    params.iter().enumerate().find_map(|(idx, param)| {
        let Pat::Ident(binding) = &param.pat else {
            return None;
        };
        if same_ident(&binding.id, ident) {
            Some((idx, binding.id.clone()))
        } else {
            None
        }
    })
}

fn find_plain_arrow_param_ident(params: &[Pat], ident: &Ident) -> Option<(usize, Ident)> {
    params.iter().enumerate().find_map(|(idx, param)| {
        let Pat::Ident(binding) = param else {
            return None;
        };
        if same_ident(&binding.id, ident) {
            Some((idx, binding.id.clone()))
        } else {
            None
        }
    })
}

fn default_references_current_or_later_param(
    params: &[Param],
    idx: usize,
    default_val: &Expr,
) -> bool {
    let blocked = current_or_later_param_bindings(params, idx);
    expr_references_any_binding(default_val, &blocked)
}

fn default_references_current_or_later_arrow_param(
    params: &[Pat],
    idx: usize,
    default_val: &Expr,
) -> bool {
    let blocked = current_or_later_arrow_param_bindings(params, idx);
    expr_references_any_binding(default_val, &blocked)
}

fn default_references_blocked_param_binding(
    params: &[Param],
    idx: usize,
    default_val: &Expr,
    body_bindings: &[BindingId],
) -> bool {
    default_references_current_or_later_param(params, idx, default_val)
        || expr_references_any_binding(default_val, body_bindings)
}

fn default_references_blocked_arrow_binding(
    params: &[Pat],
    idx: usize,
    default_val: &Expr,
    body_bindings: &[BindingId],
) -> bool {
    default_references_current_or_later_arrow_param(params, idx, default_val)
        || expr_references_any_binding(default_val, body_bindings)
}

fn current_or_later_param_bindings(params: &[Param], idx: usize) -> Vec<BindingId> {
    let mut bindings = Vec::new();
    for param in params.iter().skip(idx) {
        collect_pat_binding_ids(&param.pat, &mut bindings);
    }
    bindings
}

fn current_or_later_arrow_param_bindings(params: &[Pat], idx: usize) -> Vec<BindingId> {
    let mut bindings = Vec::new();
    for param in params.iter().skip(idx) {
        collect_pat_binding_ids(param, &mut bindings);
    }
    bindings
}

fn collect_pat_binding_ids(pat: &Pat, out: &mut Vec<BindingId>) {
    match pat {
        Pat::Ident(binding) => out.push((binding.id.sym.clone(), binding.id.ctxt)),
        Pat::Assign(assign) => collect_pat_binding_ids(&assign.left, out),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_binding_ids(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_binding_ids(&kv.value, out),
                    ObjectPatProp::Assign(assign) => {
                        out.push((assign.key.id.sym.clone(), assign.key.id.ctxt));
                    }
                    ObjectPatProp::Rest(rest) => collect_pat_binding_ids(&rest.arg, out),
                }
            }
        }
        Pat::Rest(rest) => collect_pat_binding_ids(&rest.arg, out),
        _ => {}
    }
}

fn collect_body_bindings(body: &BlockStmt) -> Vec<BindingId> {
    let mut collector = BodyBindingCollector {
        bindings: Vec::new(),
    };
    body.visit_with(&mut collector);
    collector.bindings
}

struct BodyBindingCollector {
    bindings: Vec<BindingId>,
}

impl Visit for BodyBindingCollector {
    fn visit_var_decl(&mut self, var: &VarDecl) {
        for decl in &var.decls {
            collect_pat_binding_ids(&decl.name, &mut self.bindings);
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.bindings
            .push((decl.ident.sym.clone(), decl.ident.ctxt));
    }

    fn visit_class_decl(&mut self, decl: &ClassDecl) {
        self.bindings
            .push((decl.ident.sym.clone(), decl.ident.ctxt));
    }

    fn visit_catch_clause(&mut self, clause: &CatchClause) {
        if let Some(param) = &clause.param {
            collect_pat_binding_ids(param, &mut self.bindings);
        }
        clause.body.visit_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn expr_references_any_binding(expr: &Expr, bindings: &[BindingId]) -> bool {
    if bindings.is_empty() {
        return false;
    }
    let mut finder = BindingReferenceFinder {
        bindings,
        found: false,
    };
    expr.visit_with(&mut finder);
    finder.found
}

struct BindingReferenceFinder<'a> {
    bindings: &'a [BindingId],
    found: bool,
}

impl Visit for BindingReferenceFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if self
            .bindings
            .iter()
            .any(|(sym, ctxt)| *sym == ident.sym && *ctxt == ident.ctxt)
        {
            self.found = true;
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn set_param_default(
    params: &mut [Param],
    idx: usize,
    default_val: Box<Expr>,
    body_bindings: &[BindingId],
) -> bool {
    if default_references_blocked_param_binding(params, idx, &default_val, body_bindings) {
        return false;
    }
    let Pat::Ident(binding) = &params[idx].pat else {
        return false;
    };
    let binding = binding.clone();
    params[idx].pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(binding)),
        right: default_val,
    });
    true
}

fn set_arrow_param_default(
    params: &mut [Pat],
    idx: usize,
    default_val: Box<Expr>,
    body_bindings: &[BindingId],
) -> bool {
    if default_references_blocked_arrow_binding(params, idx, &default_val, body_bindings) {
        return false;
    }
    let Pat::Ident(binding) = &params[idx] else {
        return false;
    };
    let binding = binding.clone();
    params[idx] = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(binding)),
        right: default_val,
    });
    true
}

fn is_empty_object_literal(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Object(obj) if obj.props.is_empty())
}

fn is_empty_array_literal(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Array(array) if array.elems.is_empty())
}

fn is_ident_expr(expr: &Expr, ident: &Ident) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if same_ident(id, ident))
}

// ============================================================
// Destructured parameter alias folding
// ============================================================

fn fold_destructured_param_aliases(
    params: &mut [Param],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
) {
    loop {
        let destructuring_idx = leading_bare_decl_count(&body.stmts);
        let Some((alias, destructured_pat, default_val)) = body
            .stmts
            .get(destructuring_idx)
            .and_then(|stmt| extract_destructuring_alias_stmt(stmt, unresolved_mark))
        else {
            break;
        };
        let Some(param_idx) = find_param_alias_idx(params, &alias) else {
            break;
        };
        if destructured_pat_references_alias(&destructured_pat, &alias)
            || destructured_pat_has_minified_alias(&destructured_pat)
            || destructured_pat_references_later_decl_name(
                &destructured_pat,
                &body.stmts[..destructuring_idx],
            )
            || destructured_pat_references_later_decl_name(
                &destructured_pat,
                &body.stmts[destructuring_idx + 1..],
            )
            || stmts_reference_ident(&body.stmts[destructuring_idx + 1..], &alias)
            || destructured_pat_reuses_other_param_name(&destructured_pat, params, param_idx)
            || !replace_param_alias_pat(
                &mut params[param_idx].pat,
                &alias,
                destructured_pat,
                default_val,
            )
        {
            break;
        }
        body.stmts.remove(destructuring_idx);
    }
}

// ============================================================
// Promote reassignment defaults on destructured bindings
// ============================================================
//
// Babel 7.8 emits nested destructuring defaults as a body reassignment:
//   function f({ outer: binding } = {}) {
//       binding = binding === void 0 ? {} : binding;
//       let temp = binding.prop;
//       let local = temp === void 0 ? val : temp;
//   }
// Newer Babel versions use a fresh temp in a VarDecl, which the existing
// fold functions handle. This step covers the reassignment form, promoting
// the default into the pattern and optionally folding property accesses
// into a nested destructuring.

fn promote_destructured_binding_defaults(
    params: &mut [Param],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
) -> bool {
    let scan_limit = body.stmts.len().min(10);

    for stmt_idx in 0..scan_limit {
        if stmt_idx >= body.stmts.len() {
            break;
        }

        let Some((binding, default_val)) =
            extract_reassignment_default(&body.stmts[stmt_idx], unresolved_mark)
        else {
            continue;
        };

        if !any_param_contains_destructured_binding(params, &binding) {
            continue;
        }

        body.stmts.remove(stmt_idx);

        // Try to fold property accesses on the binding into a nested
        // destructuring pattern. Only attempt this when the property accesses
        // are at the front of the remaining body (the common case after
        // earlier fold passes have run).
        if stmt_idx == 0 {
            if let Some((props, _, remove_count)) =
                extract_object_property_alias_props(body, &binding, unresolved_mark)
            {
                if !stmts_reference_ident(&body.stmts[remove_count..], &binding) {
                    let nested_pat = Pat::Object(ObjectPat {
                        span: DUMMY_SP,
                        props,
                        optional: false,
                        type_ann: None,
                    });
                    let with_default = Pat::Assign(AssignPat {
                        span: DUMMY_SP,
                        left: Box::new(nested_pat),
                        right: default_val,
                    });
                    replace_destructured_binding_in_params(params, &binding, with_default);
                    body.stmts.drain(0..remove_count);
                    return true;
                }
            }
        }

        // No property folding possible — just promote the default.
        let default_pat = Pat::Assign(AssignPat {
            span: DUMMY_SP,
            left: Box::new(Pat::Ident(BindingIdent {
                id: binding.clone(),
                type_ann: None,
            })),
            right: default_val,
        });
        replace_destructured_binding_in_params(params, &binding, default_pat);
        return true;
    }

    false
}

fn extract_reassignment_default(stmt: &Stmt, unresolved_mark: Mark) -> Option<(Ident, Box<Expr>)> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) = &assign.left else {
        return None;
    };

    let Expr::Cond(cond) = strip_parens(assign.right.as_ref()) else {
        return None;
    };
    let checked = extract_void0_check(cond.test.as_ref(), unresolved_mark)?;
    if !same_ident(&checked, &binding.id) {
        return None;
    }
    if !is_ident_expr(cond.alt.as_ref(), &binding.id) {
        return None;
    }

    if !is_empty_object_literal(cond.cons.as_ref()) && !is_empty_array_literal(cond.cons.as_ref()) {
        return None;
    }

    Some((binding.id.clone(), cond.cons.clone()))
}

fn any_param_contains_destructured_binding(params: &[Param], binding: &Ident) -> bool {
    params
        .iter()
        .any(|param| pat_contains_destructured_binding(&param.pat, binding))
}

fn pat_contains_destructured_binding(pat: &Pat, binding: &Ident) -> bool {
    match pat {
        Pat::Object(obj) => obj.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => kv_value_is_or_contains_binding(&kv.value, binding),
            _ => false,
        }),
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| pat_is_or_contains_binding(elem, binding)),
        Pat::Assign(assign) => pat_contains_destructured_binding(&assign.left, binding),
        _ => false,
    }
}

fn kv_value_is_or_contains_binding(pat: &Pat, binding: &Ident) -> bool {
    match pat {
        Pat::Ident(b) => same_ident(&b.id, binding),
        Pat::Assign(assign) => kv_value_is_or_contains_binding(&assign.left, binding),
        Pat::Object(_) | Pat::Array(_) => pat_contains_destructured_binding(pat, binding),
        _ => false,
    }
}

fn pat_is_or_contains_binding(pat: &Pat, binding: &Ident) -> bool {
    match pat {
        Pat::Ident(b) => same_ident(&b.id, binding),
        _ => pat_contains_destructured_binding(pat, binding),
    }
}

fn replace_destructured_binding_in_params(params: &mut [Param], binding: &Ident, replacement: Pat) {
    for param in params.iter_mut() {
        if replace_destructured_binding_in_pat(&mut param.pat, binding, &replacement) {
            return;
        }
    }
}

fn replace_destructured_binding_in_pat(pat: &mut Pat, binding: &Ident, replacement: &Pat) -> bool {
    match pat {
        Pat::Object(obj) => {
            for prop in &mut obj.props {
                if let ObjectPatProp::KeyValue(kv) = prop {
                    if replace_kv_value_binding(&mut kv.value, binding, replacement) {
                        return true;
                    }
                }
            }
            false
        }
        Pat::Array(array) => {
            for elem in array.elems.iter_mut().flatten() {
                if replace_pat_binding(elem, binding, replacement) {
                    return true;
                }
            }
            false
        }
        Pat::Assign(assign) => {
            replace_destructured_binding_in_pat(&mut assign.left, binding, replacement)
        }
        _ => false,
    }
}

fn replace_kv_value_binding(pat: &mut Pat, binding: &Ident, replacement: &Pat) -> bool {
    match pat {
        Pat::Ident(b) if same_ident(&b.id, binding) => {
            *pat = replacement.clone();
            true
        }
        Pat::Assign(assign) => {
            if let Pat::Ident(b) = assign.left.as_ref() {
                if same_ident(&b.id, binding) {
                    *pat = replacement.clone();
                    return true;
                }
            }
            replace_destructured_binding_in_pat(pat, binding, replacement)
        }
        _ => replace_destructured_binding_in_pat(pat, binding, replacement),
    }
}

fn replace_pat_binding(pat: &mut Pat, binding: &Ident, replacement: &Pat) -> bool {
    match pat {
        Pat::Ident(b) if same_ident(&b.id, binding) => {
            *pat = replacement.clone();
            true
        }
        _ => replace_destructured_binding_in_pat(pat, binding, replacement),
    }
}

fn leading_bare_decl_count(stmts: &[Stmt]) -> usize {
    stmts.iter().take_while(|stmt| is_bare_decl(stmt)).count()
}

fn is_bare_decl(stmt: &Stmt) -> bool {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return false;
    };
    if var.kind == VarDeclKind::Const {
        return false;
    }
    var.decls
        .iter()
        .all(|decl| decl.init.is_none() && matches!(decl.name, Pat::Ident(_)))
}

fn fold_object_property_param_aliases(
    params: &mut [Param],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
) {
    loop {
        let Some((param_idx, alias, mut destructured_pat, default_val, remove_count)) =
            extract_prefix_object_property_aliases(params, body, unresolved_mark)
        else {
            break;
        };
        if !computed_object_param_keys_are_safe(params, param_idx, &destructured_pat) {
            break;
        }
        let short_alias_renames = rename_short_object_property_aliases(
            params,
            param_idx,
            &mut destructured_pat,
            &body.stmts[remove_count..],
        );
        if destructured_pat_references_alias(&destructured_pat, &alias)
            || destructured_pat_references_later_decl_name(
                &destructured_pat,
                &body.stmts[remove_count..],
            )
            || stmts_reference_ident(&body.stmts[remove_count..], &alias)
            || destructured_pat_reuses_other_param_name(&destructured_pat, params, param_idx)
            || !replace_param_alias_pat(
                &mut params[param_idx].pat,
                &alias,
                destructured_pat,
                default_val,
            )
        {
            break;
        }
        body.stmts.drain(0..remove_count);
        rename_bindings(&mut body.stmts, &short_alias_renames);
    }
}

fn fold_array_index_param_aliases(
    params: &mut [Param],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
) {
    loop {
        let Some((param_idx, alias, destructured_pat, default_val, remove_count)) =
            extract_prefix_array_index_aliases(params, body, unresolved_mark)
        else {
            break;
        };
        if destructured_pat_references_alias(&destructured_pat, &alias)
            || destructured_pat_references_later_decl_name(
                &destructured_pat,
                &body.stmts[remove_count..],
            )
            || stmts_reference_ident(&body.stmts[remove_count..], &alias)
            || destructured_pat_reuses_other_param_name(&destructured_pat, params, param_idx)
            || !replace_param_alias_pat(
                &mut params[param_idx].pat,
                &alias,
                destructured_pat,
                default_val,
            )
        {
            break;
        }
        body.stmts.drain(0..remove_count);
    }
}

fn fold_destructured_arrow_param_aliases(
    params: &mut [Pat],
    body: &mut BlockStmt,
    unresolved_mark: Mark,
) {
    loop {
        let Some((alias, destructured_pat, default_val)) =
            extract_prefix_destructuring_alias(body, unresolved_mark)
        else {
            break;
        };
        let Some(param_idx) = find_arrow_param_alias_idx(params, &alias) else {
            break;
        };
        if destructured_pat_references_alias(&destructured_pat, &alias)
            || destructured_pat_has_minified_alias(&destructured_pat)
            || destructured_pat_references_later_decl_name(&destructured_pat, &body.stmts[1..])
            || stmts_reference_ident(&body.stmts[1..], &alias)
            || destructured_pat_reuses_other_arrow_param_name(&destructured_pat, params, param_idx)
            || !replace_param_alias_pat(
                &mut params[param_idx],
                &alias,
                destructured_pat,
                default_val,
            )
        {
            break;
        }
        body.stmts.remove(0);
    }
}

fn extract_prefix_destructuring_alias(
    body: &BlockStmt,
    unresolved_mark: Mark,
) -> Option<(Ident, Pat, Option<Box<Expr>>)> {
    extract_destructuring_alias_stmt(body.stmts.first()?, unresolved_mark)
}

fn extract_destructuring_alias_stmt(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(Ident, Pat, Option<Box<Expr>>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let decl = &var.decls[0];
    let destructured_pat = match &decl.name {
        Pat::Object(_) | Pat::Array(_) => decl.name.clone(),
        _ => return None,
    };
    let init = strip_parens(decl.init.as_ref()?);
    if let Expr::Ident(alias) = init {
        return Some((alias.clone(), destructured_pat, None));
    }

    let (alias, default_val) = extract_destructuring_alias_default(init, unresolved_mark)?;
    Some((alias, destructured_pat, Some(default_val)))
}

fn extract_destructuring_alias_default(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<(Ident, Box<Expr>)> {
    let Expr::Cond(cond) = strip_parens(expr) else {
        return None;
    };
    let param_ident = extract_void0_check(cond.test.as_ref(), unresolved_mark)?;
    let Expr::Ident(alias) = strip_parens(cond.alt.as_ref()) else {
        return None;
    };
    if !same_ident(alias, &param_ident) {
        return None;
    }
    let default_val = cond.cons.clone();
    if !is_empty_object_literal(&default_val) && !is_empty_array_literal(&default_val) {
        return None;
    }
    Some((alias.clone(), default_val))
}

fn extract_prefix_object_property_aliases(
    params: &[Param],
    body: &BlockStmt,
    unresolved_mark: Mark,
) -> Option<(usize, Ident, Pat, Option<Box<Expr>>, usize)> {
    for (param_idx, param) in params.iter().enumerate() {
        let alias = param_alias_ident(&param.pat)?;
        let Some((props, default_val, remove_count)) =
            extract_object_property_alias_props(body, &alias, unresolved_mark)
        else {
            continue;
        };
        return Some((
            param_idx,
            alias,
            Pat::Object(ObjectPat {
                span: DUMMY_SP,
                props,
                optional: false,
                type_ann: None,
            }),
            default_val,
            remove_count,
        ));
    }
    None
}

fn extract_prefix_array_index_aliases(
    params: &[Param],
    body: &BlockStmt,
    unresolved_mark: Mark,
) -> Option<(usize, Ident, Pat, Option<Box<Expr>>, usize)> {
    for (param_idx, param) in params.iter().enumerate() {
        let alias = param_alias_ident(&param.pat)?;
        let Some((elems, default_val, remove_count)) =
            extract_array_index_alias_elems(body, &alias, unresolved_mark)
        else {
            continue;
        };
        return Some((
            param_idx,
            alias,
            Pat::Array(ArrayPat {
                span: DUMMY_SP,
                elems,
                optional: false,
                type_ann: None,
            }),
            default_val,
            remove_count,
        ));
    }
    None
}

fn param_alias_ident(param: &Pat) -> Option<Ident> {
    match param {
        Pat::Ident(binding) => Some(binding.id.clone()),
        Pat::Assign(assign) => {
            if let Pat::Ident(binding) = assign.left.as_ref() {
                Some(binding.id.clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_array_index_alias_elems(
    body: &BlockStmt,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<(Vec<Option<Pat>>, Option<Box<Expr>>, usize)> {
    let mut accesses = Vec::new();
    let mut stmt_idx = 0;
    let mut default_val = None;

    while let Some(local) = body
        .stmts
        .get(stmt_idx)
        .and_then(extract_uninit_local_ident)
    {
        if stmts_reference_ident(&body.stmts[stmt_idx + 1..], &local.id) {
            break;
        }
        stmt_idx += 1;
    }

    while stmt_idx < body.stmts.len() {
        let Some(index_alias) =
            extract_array_index_alias_stmt(&body.stmts[stmt_idx], alias, unresolved_mark)
        else {
            break;
        };
        if accesses
            .iter()
            .any(|(index, _): &(usize, Pat)| *index == index_alias.index)
        {
            break;
        }
        if default_val.is_none() {
            default_val = index_alias.default_val.clone();
        }

        if let Some(next_stmt) = body.stmts.get(stmt_idx + 1) {
            if let Some((binding, default)) =
                extract_default_from_temp_stmt(next_stmt, &index_alias.local.id, unresolved_mark)
            {
                if stmts_reference_ident(&body.stmts[stmt_idx + 2..], &index_alias.local.id) {
                    break;
                }
                accesses.push((
                    index_alias.index,
                    Pat::Assign(AssignPat {
                        span: DUMMY_SP,
                        left: Box::new(Pat::Ident(binding)),
                        right: default,
                    }),
                ));
                stmt_idx += 2;
                continue;
            }
        }

        accesses.push((index_alias.index, Pat::Ident(index_alias.local)));
        stmt_idx += 1;
    }

    if accesses.is_empty() {
        return None;
    }

    let max_index = accesses.iter().map(|(index, _)| *index).max()?;
    let mut elems = vec![None; max_index + 1];
    for (index, pat) in accesses {
        elems[index] = Some(pat);
    }
    Some((elems, default_val, stmt_idx))
}

struct ArrayIndexAlias {
    index: usize,
    local: BindingIdent,
    default_val: Option<Box<Expr>>,
}

fn extract_array_index_alias_stmt(
    stmt: &Stmt,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<ArrayIndexAlias> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(local) = &declarator.name else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = strip_parens(declarator.init.as_ref()?) else {
        return None;
    };
    let default_val = extract_array_index_alias_default(obj.as_ref(), alias, unresolved_mark)?;
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    let index = extract_num_literal(computed.expr.as_ref())?;
    Some(ArrayIndexAlias {
        index,
        local: local.clone(),
        default_val,
    })
}

fn extract_array_index_alias_default(
    obj: &Expr,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<Option<Box<Expr>>> {
    let obj = strip_parens(obj);
    if matches!(obj, Expr::Ident(obj) if same_param_alias_reference(obj, alias)) {
        return Some(None);
    }

    let (default_alias, default_val) = extract_destructuring_alias_default(obj, unresolved_mark)?;
    if !same_param_alias_reference(&default_alias, alias) || !is_empty_array_literal(&default_val) {
        return None;
    }
    Some(Some(default_val))
}

fn extract_object_property_alias_props(
    body: &BlockStmt,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<(Vec<ObjectPatProp>, Option<Box<Expr>>, usize)> {
    let mut props = Vec::new();
    let mut stmt_idx = 0;
    let mut default_val = None;

    while let Some(local) = body
        .stmts
        .get(stmt_idx)
        .and_then(extract_uninit_local_ident)
    {
        if stmts_reference_ident(&body.stmts[stmt_idx + 1..], &local.id) {
            break;
        }
        stmt_idx += 1;
    }

    while stmt_idx < body.stmts.len() {
        let Some(prop_alias) =
            extract_property_alias_stmt(&body.stmts[stmt_idx], alias, unresolved_mark)
        else {
            break;
        };
        if default_val.is_none() {
            default_val = prop_alias.default_val.clone();
        }

        if let Some(next_stmt) = body.stmts.get(stmt_idx + 1) {
            if let Some((binding, default_val)) =
                extract_default_from_temp_stmt(next_stmt, &prop_alias.local.id, unresolved_mark)
            {
                if stmts_reference_ident(&body.stmts[stmt_idx + 2..], &prop_alias.local.id) {
                    break;
                }
                props.push(object_pat_prop(prop_alias.prop, binding, Some(default_val)));
                stmt_idx += 2;
                continue;
            }
        }

        props.push(object_pat_prop(prop_alias.prop, prop_alias.local, None));
        stmt_idx += 1;
    }

    if props.is_empty() {
        None
    } else {
        Some((props, default_val, stmt_idx))
    }
}

struct PropertyAlias {
    prop: PropName,
    local: BindingIdent,
    default_val: Option<Box<Expr>>,
}

fn extract_property_alias_stmt(
    stmt: &Stmt,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<PropertyAlias> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(local) = &declarator.name else {
        return None;
    };
    let init = strip_parens(declarator.init.as_ref()?);
    let Expr::Member(MemberExpr { obj, prop, .. }) = init else {
        return None;
    };
    let default_val = extract_property_alias_default(obj.as_ref(), alias, unresolved_mark)?;
    let prop = match prop {
        MemberProp::Ident(prop) => PropName::Ident(IdentName::new(prop.sym.clone(), prop.span)),
        MemberProp::Computed(computed)
            if matches!(strip_parens(&computed.expr), Expr::Ident(_)) =>
        {
            PropName::Computed(ComputedPropName {
                span: computed.span,
                expr: computed.expr.clone(),
            })
        }
        _ => return None,
    };
    Some(PropertyAlias {
        prop,
        local: local.clone(),
        default_val,
    })
}

fn extract_property_alias_default(
    obj: &Expr,
    alias: &Ident,
    unresolved_mark: Mark,
) -> Option<Option<Box<Expr>>> {
    let obj = strip_parens(obj);
    if matches!(obj, Expr::Ident(obj) if same_param_alias_reference(obj, alias)) {
        return Some(None);
    }

    let (default_alias, default_val) = extract_destructuring_alias_default(obj, unresolved_mark)?;
    if !same_param_alias_reference(&default_alias, alias) || !is_empty_object_literal(&default_val)
    {
        return None;
    }
    Some(Some(default_val))
}

fn same_param_alias_reference(reference: &Ident, alias: &Ident) -> bool {
    same_ident(reference, alias)
        || (alias.ctxt == SyntaxContext::empty() && reference.sym == alias.sym)
}

fn extract_default_from_temp_stmt(
    stmt: &Stmt,
    temp: &Ident,
    unresolved_mark: Mark,
) -> Option<(BindingIdent, Box<Expr>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(binding) = &declarator.name else {
        return None;
    };
    let Expr::Cond(cond) = strip_parens(declarator.init.as_ref()?) else {
        return None;
    };
    let checked = extract_void0_check(cond.test.as_ref(), unresolved_mark)?;
    if !same_ident(&checked, temp) || !is_ident_expr(cond.alt.as_ref(), temp) {
        return None;
    }
    Some((binding.clone(), cond.cons.clone()))
}

fn materialize_inline_temp_defaults(body: &mut BlockStmt, unresolved_mark: Mark) {
    let mut stmt_idx = 1;
    while stmt_idx + 1 < body.stmts.len() {
        let Some(temp) = extract_member_alias_local_ident(&body.stmts[stmt_idx - 1]) else {
            stmt_idx += 1;
            continue;
        };
        let Some(local) = extract_uninit_local_ident(&body.stmts[stmt_idx]) else {
            stmt_idx += 1;
            continue;
        };
        let later = &body.stmts[stmt_idx + 1..];
        let Some(default_expr) = find_single_inline_temp_default(later, &temp, unresolved_mark)
        else {
            stmt_idx += 1;
            continue;
        };

        set_single_var_init(&mut body.stmts[stmt_idx], default_expr);
        let mut rewriter = InlineTempDefaultRewriter {
            temp: &temp,
            replacement: &local.id,
            unresolved_mark,
        };
        for stmt in &mut body.stmts[stmt_idx + 1..] {
            stmt.visit_mut_with(&mut rewriter);
        }
        stmt_idx += 1;
    }
}

fn extract_member_alias_local_ident(stmt: &Stmt) -> Option<Ident> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(local) = &declarator.name else {
        return None;
    };
    if !matches!(strip_parens(declarator.init.as_ref()?), Expr::Member(_)) {
        return None;
    }
    Some(local.id.clone())
}

fn extract_uninit_local_ident(stmt: &Stmt) -> Option<BindingIdent> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 || var.decls[0].init.is_some() {
        return None;
    }
    let Pat::Ident(local) = &var.decls[0].name else {
        return None;
    };
    Some(local.clone())
}

fn set_single_var_init(stmt: &mut Stmt, init: Box<Expr>) {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return;
    };
    if let Some(declarator) = var.decls.first_mut() {
        declarator.init = Some(init);
    }
}

fn find_single_inline_temp_default(
    stmts: &[Stmt],
    temp: &Ident,
    unresolved_mark: Mark,
) -> Option<Box<Expr>> {
    let mut finder = InlineTempDefaultFinder {
        temp,
        unresolved_mark,
        matched_expr: None,
        match_count: 0,
        other_temp_refs: 0,
    };
    stmts.visit_with(&mut finder);
    if finder.match_count == 1 && finder.other_temp_refs == 0 {
        finder.matched_expr
    } else {
        None
    }
}

fn extract_temp_default_expr(
    expr: &Expr,
    temp: &Ident,
    unresolved_mark: Mark,
) -> Option<Box<Expr>> {
    let Expr::Cond(cond) = strip_parens(expr) else {
        return None;
    };
    let checked = extract_void0_check(cond.test.as_ref(), unresolved_mark)?;
    if !same_ident(&checked, temp) || !is_ident_expr(cond.alt.as_ref(), temp) {
        return None;
    }
    Some(Box::new(expr.clone()))
}

struct InlineTempDefaultFinder<'a> {
    temp: &'a Ident,
    unresolved_mark: Mark,
    matched_expr: Option<Box<Expr>>,
    match_count: usize,
    other_temp_refs: usize,
}

impl Visit for InlineTempDefaultFinder<'_> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Some(default_expr) = extract_temp_default_expr(expr, self.temp, self.unresolved_mark)
        {
            self.match_count += 1;
            self.matched_expr = Some(default_expr);
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if same_ident(ident, self.temp) {
            self.other_temp_refs += 1;
        }
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

struct InlineTempDefaultRewriter<'a> {
    temp: &'a Ident,
    replacement: &'a Ident,
    unresolved_mark: Mark,
}

impl VisitMut for InlineTempDefaultRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if extract_temp_default_expr(expr, self.temp, self.unresolved_mark).is_some() {
            let original_span = expr.span();
            let mut replacement = self.replacement.clone();
            if original_span.lo.0 != 0 {
                replacement.span = original_span;
            }
            *expr = Expr::Ident(replacement);
            return;
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, _: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}

fn object_pat_prop(
    prop: PropName,
    binding: BindingIdent,
    default_val: Option<Box<Expr>>,
) -> ObjectPatProp {
    let value_pat = if let Some(default_val) = default_val {
        Pat::Assign(AssignPat {
            span: DUMMY_SP,
            left: Box::new(Pat::Ident(binding.clone())),
            right: default_val,
        })
    } else {
        Pat::Ident(binding.clone())
    };

    if matches!(&prop, PropName::Ident(key) if binding.id.sym == key.sym) {
        let value = match value_pat {
            Pat::Assign(assign) => Some(assign.right),
            _ => None,
        };
        ObjectPatProp::Assign(AssignPatProp {
            span: DUMMY_SP,
            key: binding,
            value,
        })
    } else {
        ObjectPatProp::KeyValue(KeyValuePatProp {
            key: prop,
            value: Box::new(value_pat),
        })
    }
}

fn computed_object_param_keys_are_safe(params: &[Param], param_idx: usize, pat: &Pat) -> bool {
    let mut computed_keys = Vec::new();
    if !collect_computed_param_key_ids(pat, &mut computed_keys) {
        return false;
    }
    if computed_keys.is_empty() {
        return true;
    }

    // The original body access runs after every parameter initializer. Keep
    // the destructured alias last so moving its key evaluation into the
    // parameter list cannot move it before a later initializer side effect.
    if param_idx + 1 != params.len() {
        return false;
    }

    let mut earlier_bindings = Vec::new();
    for param in &params[..param_idx] {
        collect_pat_binding_ids(&param.pat, &mut earlier_bindings);
    }
    computed_keys
        .iter()
        .all(|key| earlier_bindings.contains(key))
}

fn collect_computed_param_key_ids(pat: &Pat, out: &mut Vec<BindingId>) -> bool {
    match pat {
        Pat::Assign(assign) => collect_computed_param_key_ids(&assign.left, out),
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .all(|elem| collect_computed_param_key_ids(elem, out)),
        Pat::Object(object) => object.props.iter().all(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => {
                if let PropName::Computed(computed) = &kv.key {
                    let Expr::Ident(key) = strip_parens(&computed.expr) else {
                        return false;
                    };
                    out.push((key.sym.clone(), key.ctxt));
                }
                collect_computed_param_key_ids(&kv.value, out)
            }
            ObjectPatProp::Assign(_) => true,
            ObjectPatProp::Rest(rest) => collect_computed_param_key_ids(&rest.arg, out),
        }),
        Pat::Rest(rest) => collect_computed_param_key_ids(&rest.arg, out),
        _ => true,
    }
}

fn rename_short_object_property_aliases(
    params: &[Param],
    param_idx: usize,
    pat: &mut Pat,
    later_stmts: &[Stmt],
) -> Vec<BindingRename> {
    let Pat::Object(object) = pat else {
        return Vec::new();
    };

    let mut renames = Vec::new();
    let mut reserved_names = Vec::new();
    for (idx, param) in params.iter().enumerate() {
        if idx != param_idx {
            collect_pat_bound_emitted_names(&param.pat, &mut reserved_names);
        }
    }

    for prop in &mut object.props {
        let Some((old, new, replacement)) =
            rename_short_object_property_alias_prop(prop, later_stmts, &reserved_names)
        else {
            continue;
        };
        *prop = replacement;
        reserved_names.push(new.clone());
        renames.push(BindingRename { old, new });
    }

    renames
}

fn rename_short_object_property_alias_prop(
    prop: &mut ObjectPatProp,
    later_stmts: &[Stmt],
    reserved_names: &[Atom],
) -> Option<(BindingId, Atom, ObjectPatProp)> {
    let ObjectPatProp::KeyValue(kv) = prop else {
        return None;
    };
    let PropName::Ident(key) = &kv.key else {
        return None;
    };
    let key_sym = key.sym.clone();
    let binding = key_value_pat_binding_mut(kv)?;
    if !is_short_alias_for_key(&key_sym, &binding.id.sym)
        || !is_preferred_short_alias_target(&key_sym)
        || is_reserved_binding_name(key_sym.as_ref())
        || reserved_names.iter().any(|name| name == &key_sym)
        || stmts_contain_emitted_ident_name(later_stmts, &key_sym)
        || binding_used_as_named_object_value(later_stmts, &binding.id)
    {
        return None;
    }

    let old = (binding.id.sym.clone(), binding.id.ctxt);
    binding.id.sym = key_sym.clone();
    let replacement = ObjectPatProp::Assign(AssignPatProp {
        span: DUMMY_SP,
        key: binding.clone(),
        value: key_value_pat_default(kv),
    });
    Some((old, key_sym, replacement))
}

fn key_value_pat_binding_mut(kv: &mut KeyValuePatProp) -> Option<&mut BindingIdent> {
    match kv.value.as_mut() {
        Pat::Ident(binding) => Some(binding),
        Pat::Assign(assign) => match assign.left.as_mut() {
            Pat::Ident(binding) => Some(binding),
            _ => None,
        },
        _ => None,
    }
}

fn key_value_pat_default(kv: &KeyValuePatProp) -> Option<Box<Expr>> {
    match kv.value.as_ref() {
        Pat::Assign(assign) => Some(assign.right.clone()),
        _ => None,
    }
}

fn find_param_alias_idx(params: &[Param], alias: &Ident) -> Option<usize> {
    params
        .iter()
        .position(|param| param_pat_matches_alias(&param.pat, alias))
}

fn find_arrow_param_alias_idx(params: &[Pat], alias: &Ident) -> Option<usize> {
    params
        .iter()
        .position(|param| param_pat_matches_alias(param, alias))
}

fn param_pat_matches_alias(param: &Pat, alias: &Ident) -> bool {
    match param {
        Pat::Ident(binding) => same_ident(&binding.id, alias),
        Pat::Assign(assign) => {
            matches!(assign.left.as_ref(), Pat::Ident(binding) if same_ident(&binding.id, alias))
        }
        _ => false,
    }
}

fn replace_param_alias_pat(
    param: &mut Pat,
    alias: &Ident,
    destructured_pat: Pat,
    default_val: Option<Box<Expr>>,
) -> bool {
    match param {
        Pat::Ident(binding) if same_ident(&binding.id, alias) => {
            *param = if let Some(default_val) = default_val {
                Pat::Assign(AssignPat {
                    span: DUMMY_SP,
                    left: Box::new(destructured_pat),
                    right: default_val,
                })
            } else {
                destructured_pat
            };
            true
        }
        Pat::Assign(assign)
            if default_val.is_none()
                && matches!(assign.left.as_ref(), Pat::Ident(binding) if same_ident(&binding.id, alias)) =>
        {
            *assign.left = destructured_pat;
            true
        }
        _ => false,
    }
}

fn destructured_pat_reuses_other_param_name(
    destructured_pat: &Pat,
    params: &[Param],
    param_idx: usize,
) -> bool {
    let mut destructured_bindings = Vec::new();
    collect_pat_bound_emitted_names(destructured_pat, &mut destructured_bindings);
    params.iter().enumerate().any(|(idx, param)| {
        idx != param_idx && pat_binds_any_emitted_name(&param.pat, &destructured_bindings)
    })
}

fn destructured_pat_reuses_other_arrow_param_name(
    destructured_pat: &Pat,
    params: &[Pat],
    param_idx: usize,
) -> bool {
    let mut destructured_bindings = Vec::new();
    collect_pat_bound_emitted_names(destructured_pat, &mut destructured_bindings);
    params.iter().enumerate().any(|(idx, param)| {
        idx != param_idx && pat_binds_any_emitted_name(param, &destructured_bindings)
    })
}

fn pat_binds_any_emitted_name(pat: &Pat, names: &[Atom]) -> bool {
    let mut existing = Vec::new();
    collect_pat_bound_emitted_names(pat, &mut existing);
    existing.iter().any(|name| names.iter().any(|n| n == name))
}

// These collision checks intentionally use emitted names, not SyntaxContext.
// Moving a destructuring declaration into the parameter list changes where names
// are declared, so two distinct bindings with the same printed name can become an
// invalid or meaningfully different parameter list after the rewrite.
fn collect_pat_bound_emitted_names(pat: &Pat, out: &mut Vec<Atom>) {
    match pat {
        Pat::Ident(binding) => out.push(binding.id.sym.clone()),
        Pat::Assign(assign) => collect_pat_bound_emitted_names(&assign.left, out),
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_bound_emitted_names(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => collect_pat_bound_emitted_names(&kv.value, out),
                    ObjectPatProp::Assign(assign) => out.push(assign.key.id.sym.clone()),
                    ObjectPatProp::Rest(rest) => collect_pat_bound_emitted_names(&rest.arg, out),
                }
            }
        }
        Pat::Rest(rest) => collect_pat_bound_emitted_names(&rest.arg, out),
        _ => {}
    }
}

fn destructured_pat_references_alias(pat: &Pat, alias: &Ident) -> bool {
    match pat {
        Pat::Assign(assign) => {
            expr_references_ident(&assign.right, alias)
                || destructured_pat_references_alias(&assign.left, alias)
        }
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(|elem| destructured_pat_references_alias(elem, alias)),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => {
                prop_name_references_ident(&kv.key, alias)
                    || destructured_pat_references_alias(&kv.value, alias)
            }
            ObjectPatProp::Assign(assign) => assign
                .value
                .as_ref()
                .is_some_and(|value| expr_references_ident(value, alias)),
            ObjectPatProp::Rest(rest) => destructured_pat_references_alias(&rest.arg, alias),
        }),
        Pat::Rest(rest) => destructured_pat_references_alias(&rest.arg, alias),
        _ => false,
    }
}

fn destructured_pat_has_minified_alias(pat: &Pat) -> bool {
    match pat {
        Pat::Assign(assign) => destructured_pat_has_minified_alias(&assign.left),
        Pat::Array(array) => array
            .elems
            .iter()
            .flatten()
            .any(destructured_pat_has_minified_alias),
        Pat::Object(object) => object.props.iter().any(|prop| match prop {
            ObjectPatProp::KeyValue(kv) => {
                key_value_pat_has_minified_alias(kv)
                    || destructured_pat_has_minified_alias(&kv.value)
            }
            ObjectPatProp::Assign(_) => false,
            ObjectPatProp::Rest(rest) => destructured_pat_has_minified_alias(&rest.arg),
        }),
        Pat::Rest(rest) => destructured_pat_has_minified_alias(&rest.arg),
        _ => false,
    }
}

fn key_value_pat_has_minified_alias(kv: &swc_core::ecma::ast::KeyValuePatProp) -> bool {
    let PropName::Ident(key) = &kv.key else {
        return false;
    };
    match kv.value.as_ref() {
        Pat::Ident(binding) => is_short_alias_for_key(&key.sym, &binding.id.sym),
        Pat::Assign(assign) => {
            matches!(assign.left.as_ref(), Pat::Ident(binding) if is_short_alias_for_key(&key.sym, &binding.id.sym))
        }
        _ => false,
    }
}

fn is_short_alias_for_key(key: &Atom, alias: &Atom) -> bool {
    key != alias && is_likely_generated_alias(alias)
}

fn is_preferred_short_alias_target(key: &Atom) -> bool {
    matches!(key.as_ref(), "type" | "kind" | "name" | "key")
}

fn stmts_contain_emitted_ident_name(stmts: &[Stmt], name: &Atom) -> bool {
    let mut finder = EmittedIdentNameFinder2 { name, found: false };
    stmts.visit_with(&mut finder);
    finder.found
}

struct EmittedIdentNameFinder2<'a> {
    name: &'a Atom,
    found: bool,
}

impl Visit for EmittedIdentNameFinder2<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == *self.name {
            self.found = true;
        }
    }

    fn visit_prop_name(&mut self, _: &PropName) {}

    fn visit_member_prop(&mut self, prop: &MemberProp) {
        if let MemberProp::Computed(computed) = prop {
            computed.visit_children_with(self);
        }
    }
}

fn binding_used_as_named_object_value(stmts: &[Stmt], binding: &Ident) -> bool {
    let mut finder = NamedObjectValueFinder {
        binding,
        found: false,
    };
    stmts.visit_with(&mut finder);
    finder.found
}

struct NamedObjectValueFinder<'a> {
    binding: &'a Ident,
    found: bool,
}

impl Visit for NamedObjectValueFinder<'_> {
    fn visit_prop(&mut self, prop: &Prop) {
        if let Prop::KeyValue(key_value) = prop {
            if let (PropName::Ident(key), Expr::Ident(value)) =
                (&key_value.key, key_value.value.as_ref())
            {
                if key.sym != self.binding.sym
                    && key.sym.len() > 2
                    && same_ident(value, self.binding)
                {
                    self.found = true;
                    return;
                }
            }
        }
        prop.visit_children_with(self);
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn destructured_pat_references_later_decl_name(pat: &Pat, later_stmts: &[Stmt]) -> bool {
    let mut referenced = Vec::new();
    collect_pat_expr_reference_emitted_names(pat, &mut referenced);
    if referenced.is_empty() {
        return false;
    }

    let mut later_bindings = Vec::new();
    collect_direct_decl_emitted_names(later_stmts, &mut later_bindings);
    referenced
        .iter()
        .any(|name| later_bindings.iter().any(|binding| binding == name))
}

// This is also an emitted-name check: moving pattern defaults into parameter
// scope can make a default expression resolve before a later body declaration.
fn collect_pat_expr_reference_emitted_names(pat: &Pat, out: &mut Vec<Atom>) {
    match pat {
        Pat::Assign(assign) => {
            collect_expr_reference_emitted_names(&assign.right, out);
            collect_pat_expr_reference_emitted_names(&assign.left, out);
        }
        Pat::Array(array) => {
            for elem in array.elems.iter().flatten() {
                collect_pat_expr_reference_emitted_names(elem, out);
            }
        }
        Pat::Object(object) => {
            for prop in &object.props {
                match prop {
                    ObjectPatProp::KeyValue(kv) => {
                        collect_prop_name_reference_emitted_names(&kv.key, out);
                        collect_pat_expr_reference_emitted_names(&kv.value, out);
                    }
                    ObjectPatProp::Assign(assign) => {
                        if let Some(value) = &assign.value {
                            collect_expr_reference_emitted_names(value, out);
                        }
                    }
                    ObjectPatProp::Rest(rest) => {
                        collect_pat_expr_reference_emitted_names(&rest.arg, out)
                    }
                }
            }
        }
        Pat::Rest(rest) => collect_pat_expr_reference_emitted_names(&rest.arg, out),
        _ => {}
    }
}

fn collect_prop_name_reference_emitted_names(prop: &PropName, out: &mut Vec<Atom>) {
    if let PropName::Computed(computed) = prop {
        collect_expr_reference_emitted_names(&computed.expr, out);
    }
}

fn collect_expr_reference_emitted_names(expr: &Expr, out: &mut Vec<Atom>) {
    let mut visitor = EmittedIdentNameCollector { names: out };
    expr.visit_with(&mut visitor);
}

fn collect_direct_decl_emitted_names(stmts: &[Stmt], out: &mut Vec<Atom>) {
    for stmt in stmts {
        let Stmt::Decl(decl) = stmt else {
            continue;
        };
        match decl {
            Decl::Var(var) => {
                for declarator in &var.decls {
                    collect_pat_bound_emitted_names(&declarator.name, out);
                }
            }
            Decl::Fn(fn_decl) => out.push(fn_decl.ident.sym.clone()),
            Decl::Class(class_decl) => out.push(class_decl.ident.sym.clone()),
            _ => {}
        }
    }
}

struct EmittedIdentNameCollector<'a> {
    names: &'a mut Vec<Atom>,
}

impl Visit for EmittedIdentNameCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        self.names.push(ident.sym.clone());
    }

    fn visit_function(&mut self, _: &Function) {}

    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn prop_name_references_ident(prop: &PropName, alias: &Ident) -> bool {
    matches!(prop, PropName::Computed(computed) if expr_references_ident(&computed.expr, alias))
}

fn stmts_reference_ident(stmts: &[Stmt], alias: &Ident) -> bool {
    let mut visitor = IdentReferenceFinder {
        alias,
        found: false,
    };
    stmts.visit_with(&mut visitor);
    visitor.found
}

fn expr_references_ident(expr: &Expr, alias: &Ident) -> bool {
    let mut visitor = IdentReferenceFinder {
        alias,
        found: false,
    };
    expr.visit_with(&mut visitor);
    visitor.found
}

struct IdentReferenceFinder<'a> {
    alias: &'a Ident,
    found: bool,
}

impl Visit for IdentReferenceFinder<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        if same_ident(ident, self.alias) {
            self.found = true;
        }
    }
}

// ============================================================
// Pattern B: arguments-based default params
// ============================================================

fn process_pattern_b_params(
    params: &mut Vec<Param>,
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let mut to_remove: Vec<usize> = Vec::new();

    // Scan entire body for var declarations matching the arguments pattern
    for (stmt_idx, stmt) in body.stmts.iter().enumerate() {
        if let Some((param_idx, param_ident, default_val)) =
            extract_arguments_default(stmt, unresolved_mark)
        {
            if param_slot_can_use_ident(params, param_idx, &param_ident)
                && ensure_default_param(params, param_idx, param_ident, default_val, body_bindings)
                    .is_some()
            {
                to_remove.push(stmt_idx);
            }
            continue;
        }

        if let Some((param_idx, param_ident)) = extract_arguments_alias(stmt, unresolved_mark) {
            if param_slot_can_use_ident(params, param_idx, &param_ident)
                && ensure_plain_param(params, param_idx, param_ident).is_some()
            {
                to_remove.push(stmt_idx);
            }
        }
    }

    for idx in to_remove.into_iter().rev() {
        body.stmts.remove(idx);
    }
}

/// Match:
/// `var name = arguments.length > N && arguments[N] !== undefined ? arguments[N] : defaultVal`
/// Returns `(param_index, var_name, default_value)` if matched.
fn extract_arguments_default(
    stmt: &Stmt,
    unresolved_mark: Mark,
) -> Option<(usize, Ident, Box<Expr>)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    // Only var/let declarations with a single declarator
    if var.kind == VarDeclKind::Const {
        return None;
    }
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(BindingIdent { id: var_ident, .. }) = &declarator.name else {
        return None;
    };
    let init = declarator.init.as_ref()?;

    let (param_idx, default_val) = extract_arguments_default_expr(init.as_ref(), unresolved_mark)?;
    Some((param_idx, var_ident.clone(), default_val))
}

/// Check if `expr` is `arguments.length > N ? arguments[N] : undefined` (simple optional)
fn extract_simple_arguments_optional(expr: &Expr, unresolved_mark: Mark) -> Option<usize> {
    let Expr::Cond(cond) = expr else {
        return None;
    };
    let n = extract_arguments_length_threshold(cond.test.as_ref(), unresolved_mark)?;
    if !is_arguments_idx(&cond.cons, n, unresolved_mark) {
        return None;
    }
    if !is_void0_or_undefined(&cond.alt, unresolved_mark) {
        return None;
    }
    Some(n)
}

/// Extract index N from `arguments.length > N && arguments[N] !== undefined`
fn extract_arguments_cond_index(expr: &Expr, unresolved_mark: Mark) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };

    // Handle: `arguments.length > N && arguments[N] !== undefined`
    if *op == BinaryOp::LogicalAnd {
        let n = extract_arguments_length_threshold(left.as_ref(), unresolved_mark)?;

        // Right side: `arguments[N] !== undefined` or `undefined !== arguments[N]`
        let Expr::Bin(BinExpr {
            op: neq_op,
            left: rl,
            right: rr,
            ..
        }) = right.as_ref()
        else {
            return None;
        };
        if *neq_op != BinaryOp::NotEqEq {
            return None;
        }
        // arguments[N] !== undefined  OR  undefined !== arguments[N]
        let args_side = if is_void0_or_undefined(rr, unresolved_mark) {
            rl.as_ref()
        } else if is_void0_or_undefined(rl, unresolved_mark) {
            rr.as_ref()
        } else {
            return None;
        };
        if !is_arguments_idx(args_side, n, unresolved_mark) {
            return None;
        }
        return Some(n);
    }

    None
}

fn extract_arguments_length_threshold(expr: &Expr, unresolved_mark: Mark) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    match *op {
        BinaryOp::Gt if is_arguments_length(left.as_ref(), unresolved_mark) => {
            extract_num_literal(right.as_ref())
        }
        BinaryOp::Lt if is_arguments_length(right.as_ref(), unresolved_mark) => {
            extract_num_literal(left.as_ref())
        }
        _ => None,
    }
}

fn extract_arguments_default_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<(usize, Box<Expr>)> {
    let expr = strip_parens(expr);
    if let Some(n) = extract_simple_arguments_optional(expr, unresolved_mark) {
        let _ = n;
        return None;
    }

    match expr {
        Expr::Cond(cond) => {
            let param_idx = extract_arguments_cond_index(cond.test.as_ref(), unresolved_mark)?;
            if !is_arguments_idx(cond.cons.as_ref(), param_idx, unresolved_mark) {
                return None;
            }
            if is_void0_or_undefined(cond.alt.as_ref(), unresolved_mark) {
                return None;
            }
            Some((param_idx, cond.alt.clone()))
        }
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            let Expr::Unary(UnaryExpr {
                op: UnaryOp::Bang,
                arg,
                ..
            }) = left.as_ref()
            else {
                return None;
            };
            let param_idx = extract_arguments_cond_index(arg.as_ref(), unresolved_mark)?;
            if !is_arguments_idx(right.as_ref(), param_idx, unresolved_mark) {
                return None;
            }
            Some((
                param_idx,
                Box::new(Expr::Lit(Lit::Bool(Bool {
                    span: DUMMY_SP,
                    value: true,
                }))),
            ))
        }
        _ => None,
    }
}

fn extract_arguments_alias(stmt: &Stmt, unresolved_mark: Mark) -> Option<(usize, Ident)> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }
    let declarator = &var.decls[0];
    let Pat::Ident(BindingIdent { id: var_ident, .. }) = &declarator.name else {
        return None;
    };
    let init = declarator.init.as_ref()?;
    let param_idx = extract_arguments_index_expr(init.as_ref(), unresolved_mark)
        .or_else(|| extract_simple_arguments_optional(init.as_ref(), unresolved_mark))?;
    Some((param_idx, var_ident.clone()))
}

fn extract_arguments_index_expr(expr: &Expr, unresolved_mark: Mark) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if is_arguments_ident(id, unresolved_mark)) {
        return None;
    }
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    extract_num_literal(computed.expr.as_ref())
}

/// Check if expr is `arguments.length`
fn is_arguments_length(expr: &Expr, unresolved_mark: Mark) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if is_arguments_ident(id, unresolved_mark)) {
        return false;
    }
    matches!(prop, MemberProp::Ident(i) if i.sym == "length")
}

/// Check if expr is `arguments[N]`
fn is_arguments_idx(expr: &Expr, n: usize, unresolved_mark: Mark) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if is_arguments_ident(id, unresolved_mark)) {
        return false;
    }
    let MemberProp::Computed(computed) = prop else {
        return false;
    };
    if let Some(idx) = extract_num_literal(&computed.expr) {
        idx == n
    } else {
        false
    }
}

fn extract_num_literal(expr: &Expr) -> Option<usize> {
    if let Expr::Lit(swc_core::ecma::ast::Lit::Num(Number { value, .. })) = expr {
        if *value >= 0.0 && value.fract() == 0.0 {
            return Some(*value as usize);
        }
    }
    None
}

// ============================================================
// Helpers
// ============================================================

fn find_plain_param_idx(params: &[Param], ident: &Ident) -> Option<usize> {
    if !plain_param_name_is_unique(params, ident) {
        return None;
    }
    params.iter().position(|p| {
        if let Pat::Ident(BindingIdent { id, .. }) = &p.pat {
            same_ident(id, ident)
        } else {
            false
        }
    })
}

fn plain_param_name_is_unique(params: &[Param], ident: &Ident) -> bool {
    params
        .iter()
        .filter(|param| matches!(&param.pat, Pat::Ident(binding) if binding.id.sym == ident.sym))
        .count()
        == 1
}

fn find_plain_pat_idx(params: &[Pat], ident: &Ident) -> Option<usize> {
    params.iter().position(|p| {
        if let Pat::Ident(BindingIdent { id, .. }) = p {
            same_ident(id, ident)
        } else {
            false
        }
    })
}

fn make_ident_param(name: Atom) -> Param {
    Param {
        span: DUMMY_SP,
        decorators: Vec::new(),
        pat: Pat::Ident(BindingIdent {
            id: Ident::new_no_ctxt(name, DUMMY_SP),
            type_ann: None,
        }),
    }
}

fn rewrite_inline_arguments_defaults(
    params: &mut Vec<Param>,
    body: &mut BlockStmt,
    unresolved_mark: Mark,
    body_bindings: &[BindingId],
) {
    let param_name_candidates = collect_inline_param_name_candidates(body);
    let initial_param_count = params.len();
    let mut rewriter = InlineArgumentsDefaultRewriter {
        params,
        initial_param_count,
        unresolved_mark,
        body_bindings,
        param_name_candidates,
        consumed_param_name_bindings: Vec::new(),
    };
    body.visit_mut_with(&mut rewriter);
    remove_consumed_empty_param_name_decls(body, &rewriter.consumed_param_name_bindings);
}

fn ensure_params_len(params: &mut Vec<Param>, idx: usize) {
    while params.len() <= idx {
        let placeholder_name = format!("_param_{}", params.len());
        params.push(make_ident_param(placeholder_name.into()));
    }
}

fn placeholder_name(idx: usize) -> Atom {
    format!("_param_{}", idx).into()
}

fn is_placeholder(sym: &Atom, idx: usize) -> bool {
    *sym == placeholder_name(idx)
}

fn param_slot_can_use_name(params: &[Param], idx: usize, preferred_name: &Atom) -> bool {
    let Some(param) = params.get(idx) else {
        return true;
    };
    let Pat::Ident(binding) = &param.pat else {
        return false;
    };
    binding.id.sym == *preferred_name || is_placeholder(&binding.id.sym, idx)
}

fn param_slot_can_use_ident(params: &[Param], idx: usize, preferred_ident: &Ident) -> bool {
    param_slot_can_use_name(params, idx, &preferred_ident.sym)
}

fn ensure_plain_param(
    params: &mut Vec<Param>,
    idx: usize,
    preferred_ident: Ident,
) -> Option<Ident> {
    ensure_params_len(params, idx);
    let Pat::Ident(binding) = &mut params[idx].pat else {
        return None;
    };
    if is_placeholder(&binding.id.sym, idx) {
        binding.id = preferred_ident;
    }
    Some(binding.id.clone())
}

fn ensure_default_param(
    params: &mut Vec<Param>,
    idx: usize,
    preferred_ident: Ident,
    default_val: Box<Expr>,
    body_bindings: &[BindingId],
) -> Option<Ident> {
    if expr_references_any_binding(&default_val, body_bindings) {
        return None;
    }
    ensure_params_len(params, idx);
    if default_references_current_or_later_param(params, idx, &default_val) {
        return None;
    }

    let (ident, can_replace) = match &params[idx].pat {
        Pat::Ident(binding) => {
            let mut id = binding.id.clone();
            if is_placeholder(&id.sym, idx) {
                id = preferred_ident;
            }
            (id, true)
        }
        _ => (preferred_ident, false),
    };

    if !can_replace {
        return None;
    }

    params[idx].pat = Pat::Assign(AssignPat {
        span: DUMMY_SP,
        left: Box::new(Pat::Ident(BindingIdent {
            id: ident.clone(),
            type_ann: None,
        })),
        right: default_val,
    });
    Some(ident)
}

#[derive(Clone)]
struct InlineParamNameCandidate {
    ident: Ident,
    binding: BindingId,
}

struct InlineArgumentsDefaultRewriter<'a> {
    params: &'a mut Vec<Param>,
    initial_param_count: usize,
    unresolved_mark: Mark,
    body_bindings: &'a [BindingId],
    param_name_candidates: Vec<Option<InlineParamNameCandidate>>,
    consumed_param_name_bindings: Vec<BindingId>,
}

impl VisitMut for InlineArgumentsDefaultRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some((idx, default_val)) = extract_arguments_default_expr(expr, self.unresolved_mark)
        {
            let original_span = expr.span();
            let preferred_ident = self.preferred_param_ident(idx);
            if param_slot_can_use_ident(self.params, idx, &preferred_ident) {
                if let Some(mut ident) = ensure_default_param(
                    self.params,
                    idx,
                    preferred_ident,
                    default_val,
                    self.body_bindings,
                ) {
                    self.mark_param_name_consumed(idx);
                    if original_span.lo.0 != 0 {
                        ident.span = original_span;
                    }
                    *expr = Expr::Ident(ident);
                }
            }
            return;
        }

        let Some(idx) = extract_simple_arguments_optional(expr, self.unresolved_mark) else {
            return;
        };

        let original_span = expr.span();
        let preferred_ident = self.preferred_param_ident(idx);
        if param_slot_can_use_ident(self.params, idx, &preferred_ident) {
            if let Some(mut ident) = ensure_plain_param(self.params, idx, preferred_ident) {
                self.mark_param_name_consumed(idx);
                if original_span.lo.0 != 0 {
                    ident.span = original_span;
                }
                *expr = Expr::Ident(ident);
            }
        }
    }

    fn visit_mut_function(&mut self, _: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}

impl InlineArgumentsDefaultRewriter<'_> {
    fn preferred_param_ident(&self, idx: usize) -> Ident {
        self.param_name_candidate(idx)
            .map(|candidate| candidate.ident.clone())
            .unwrap_or_else(|| Ident::new_no_ctxt(placeholder_name(idx), DUMMY_SP))
    }

    fn mark_param_name_consumed(&mut self, idx: usize) {
        let Some(binding) = self
            .param_name_candidate(idx)
            .map(|candidate| candidate.binding.clone())
        else {
            return;
        };
        if !self
            .consumed_param_name_bindings
            .iter()
            .any(|consumed| consumed == &binding)
        {
            self.consumed_param_name_bindings.push(binding);
        }
    }

    fn param_name_candidate(&self, idx: usize) -> Option<&InlineParamNameCandidate> {
        self.param_name_candidates
            .get(idx)
            .and_then(|candidate| candidate.as_ref())
            .or_else(|| {
                idx.checked_sub(self.initial_param_count)
                    .and_then(|offset| {
                        self.param_name_candidates
                            .get(offset)
                            .and_then(|candidate| candidate.as_ref())
                    })
            })
    }
}

fn collect_inline_param_name_candidates(body: &BlockStmt) -> Vec<Option<InlineParamNameCandidate>> {
    let mut candidates = Vec::new();

    for (stmt_idx, stmt) in body.stmts.iter().enumerate() {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            break;
        };
        if var.kind == VarDeclKind::Const {
            break;
        }

        let mut stmt_candidates = Vec::new();
        for declarator in &var.decls {
            if declarator.init.is_some() {
                return candidates;
            }
            let Pat::Ident(binding) = &declarator.name else {
                return candidates;
            };
            if stmts_reference_ident(&body.stmts[stmt_idx + 1..], &binding.id) {
                stmt_candidates.push(None);
            } else {
                stmt_candidates.push(Some(InlineParamNameCandidate {
                    ident: binding.id.clone(),
                    binding: (binding.id.sym.clone(), binding.id.ctxt),
                }));
            }
        }
        candidates.extend(stmt_candidates);
    }

    candidates
}

fn remove_consumed_empty_param_name_decls(body: &mut BlockStmt, consumed: &[BindingId]) {
    if consumed.is_empty() {
        return;
    }

    body.stmts.retain_mut(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return true;
        };
        var.decls.retain(|declarator| {
            if declarator.init.is_some() {
                return true;
            }
            let Pat::Ident(binding) = &declarator.name else {
                return true;
            };
            !consumed
                .iter()
                .any(|(sym, ctxt)| *sym == binding.id.sym && *ctxt == binding.id.ctxt)
        });
        !var.decls.is_empty()
    });
}
