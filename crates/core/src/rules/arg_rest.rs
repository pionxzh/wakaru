use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, BindingIdent, Callee, CondExpr, Constructor, Decl,
    Expr, Function, FunctionBody, Ident, Lit, MemberExpr, MemberProp, Number, Param,
    ParamOrTsParamProp, Pat, RestPat, SimpleAssignTarget, Stmt, UpdateOp, VarDecl, VarDeclKind,
    VarDeclOrExpr, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::{
    binding_id, contains_use_strict_string_statement, fresh_binding_ident,
    has_direct_use_strict_directive, ident_matches_binding, BindingId,
};
use super::helper_matcher::count_binding_refs;
use super::rename_utils::{rename_bindings, BindingRename};
use super::RewriteLevel;

/// Replaces `arguments[N]` / `arguments.length` patterns with a rest parameter
/// `...args` and rewrites safe accesses to use `args`.
///
/// Only fires when:
/// - The function does not already have a rest parameter
/// - All `arguments` usages are via subscript (`arguments[expr]`) or `.length`
/// - In functions with fixed params, the accessed indices are provably in the tail
pub struct ArgRest {
    level: RewriteLevel,
}

impl ArgRest {
    pub fn new(level: RewriteLevel) -> Self {
        Self { level }
    }
}

impl Default for ArgRest {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for ArgRest {
    fn visit_mut_function(&mut self, func: &mut Function) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        // Recurse first so inner functions are processed independently
        func.visit_mut_children_with(self);

        // Skip if already has rest params
        if func.params.iter().any(|p| matches!(p.pat, Pat::Rest(_))) {
            return;
        }

        let Some(body) = &func.body else { return };
        if has_direct_use_strict_directive(body) {
            return;
        }
        // A parameter initializer that reads `arguments` runs before the rest
        // binding exists, and a mapped index it reads would change meaning.
        if mentions_arguments(&func.params) {
            return;
        }
        let original = contains_use_strict_string_statement(body).then(|| func.clone());
        let fixed_param_count = func.params.len();

        let copy_var = detect_copy_var_ident(body, fixed_param_count);
        let mut checker = ArgumentsChecker::new(fixed_param_count);
        body.visit_with(&mut checker);

        if !checker.has_any || checker.has_unsafe {
            return;
        }

        let rest_ident = prepare_rest_ident(
            func.body.as_mut().expect("body was checked above"),
            &func.params,
            copy_var.as_ref(),
        );
        func.params.push(make_rest_param(rest_ident.clone()));

        // Rewrite `arguments` → rest param in the body
        if let Some(body) = &mut func.body {
            // Remove the Babel copy loop since the rest param replaces it
            if copy_var.is_some() {
                remove_arguments_copy_loop(body, fixed_param_count);
            }
            body.visit_mut_with(&mut ArgumentsRewriter {
                ident: rest_ident,
                fixed_param_count,
            });
        }

        if let Some(original) = original {
            if func
                .body
                .as_ref()
                .is_some_and(has_direct_use_strict_directive)
            {
                *func = original;
            }
        }
    }

    fn visit_mut_constructor(&mut self, ctor: &mut Constructor) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        ctor.visit_mut_children_with(self);

        // Skip if already has rest params
        if ctor.params.iter().any(|p| match p {
            ParamOrTsParamProp::Param(param) => matches!(param.pat, Pat::Rest(_)),
            _ => false,
        }) {
            return;
        }

        let Some(body) = &ctor.body else { return };
        if has_direct_use_strict_directive(body) {
            return;
        }
        if mentions_arguments(&ctor.params) {
            return;
        }
        let original = contains_use_strict_string_statement(body).then(|| ctor.clone());
        let fixed_param_count = ctor.params.len();

        let copy_var = detect_copy_var_ident(body, fixed_param_count);
        let mut checker = ArgumentsChecker::new(fixed_param_count);
        body.visit_with(&mut checker);

        if !checker.has_any || checker.has_unsafe {
            return;
        }

        let rest_ident = prepare_rest_ident(
            ctor.body.as_mut().expect("body was checked above"),
            &ctor.params,
            copy_var.as_ref(),
        );
        ctor.params.push(ParamOrTsParamProp::Param(make_rest_param(
            rest_ident.clone(),
        )));

        if let Some(body) = &mut ctor.body {
            // Remove the Babel copy loop since the rest param replaces it
            if copy_var.is_some() {
                remove_arguments_copy_loop(body, fixed_param_count);
            }
            body.visit_mut_with(&mut ArgumentsRewriter {
                ident: rest_ident,
                fixed_param_count,
            });
        }

        if let Some(original) = original {
            if ctor
                .body
                .as_ref()
                .is_some_and(has_direct_use_strict_directive)
            {
                *ctor = original;
            }
        }
    }
}

/// Scan `body` for the Babel rest-args copy pattern **before** `arguments` is rewritten.
/// Returns the copy variable's name (e.g. `i`, `r`, `t`) so it can be reused as the
/// rest param. Its name may still be shadowed inside a nested arrow where an
/// arguments read will be inserted; `prepare_rest_ident` checks that separately.
///
/// Pattern matched (3-declarator for-init, `arguments.length` as source):
/// ```text
/// for (var len = arguments.length, copy = Array(len), idx = 0; …) …
/// ```
fn detect_copy_var_ident(body: &FunctionBody, fixed_param_count: usize) -> Option<Ident> {
    body.stmts
        .iter()
        .find_map(|stmt| detect_copy_var_ident_from_stmt(stmt, fixed_param_count))
        .or_else(|| detect_ts_copy_var_ident(body, fixed_param_count))
}

/// Positive proof that a binding is the Array populated by a canonical
/// Babel/TypeScript rest-argument copy. `ready_stmt` is the first statement
/// after the declaration/copy loop, where the binding can be consumed as an
/// initialized Array.
pub(super) struct RestArrayCopyProof {
    pub(super) binding: Ident,
    pub(super) start_stmt: usize,
    pub(super) ready_stmt: usize,
}

/// Find a rest-array copy using resolver identity for the built-in `Array`
/// constructor and the function's implicit `arguments` binding. ArgRest's
/// historical matcher remains name-based; callers using this as an Array proof
/// need the stronger checks before replacing concat with spread.
pub(super) fn find_rest_array_copy_proof(
    body: &FunctionBody,
    fixed_param_count: usize,
    unresolved_mark: Mark,
) -> Option<RestArrayCopyProof> {
    if let Some((index, binding)) = body.stmts.iter().enumerate().find_map(|(index, stmt)| {
        detect_copy_var_ident_from_stmt_with_mark(stmt, fixed_param_count, Some(unresolved_mark))
            .filter(|_| arguments_refs_are_unresolved(stmt, unresolved_mark))
            .map(|binding| (index, binding))
    }) {
        return Some(RestArrayCopyProof {
            binding,
            start_stmt: index,
            ready_stmt: index + 1,
        });
    }

    body.stmts.windows(2).enumerate().find_map(|(index, pair)| {
        let copy_id = ts_empty_array_ident_from_stmt(&pair[0])?;
        let loop_copy = detect_ts_copy_loop_from_stmt(&pair[1], fixed_param_count)?;
        if binding_id(&copy_id) != loop_copy
            || !arguments_refs_are_unresolved(&pair[1], unresolved_mark)
        {
            return None;
        }
        Some(RestArrayCopyProof {
            binding: copy_id,
            start_stmt: index,
            ready_stmt: index + 2,
        })
    })
}

fn arguments_refs_are_unresolved<N>(node: &N, unresolved_mark: Mark) -> bool
where
    N: VisitWith<ArgumentsIdentityChecker>,
{
    let mut checker = ArgumentsIdentityChecker {
        unresolved_mark,
        valid: true,
    };
    node.visit_with(&mut checker);
    checker.valid
}

struct ArgumentsIdentityChecker {
    unresolved_mark: Mark,
    valid: bool,
}

impl Visit for ArgumentsIdentityChecker {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == "arguments" && ident.ctxt.outer() != self.unresolved_mark {
            self.valid = false;
        }
    }
}

fn detect_ts_copy_var_ident(body: &FunctionBody, fixed_param_count: usize) -> Option<Ident> {
    body.stmts.windows(2).find_map(|pair| {
        let copy_id = ts_empty_array_ident_from_stmt(&pair[0])?;
        let loop_copy = detect_ts_copy_loop_from_stmt(&pair[1], fixed_param_count)?;
        (binding_id(&copy_id) == loop_copy).then_some(copy_id)
    })
}

fn detect_copy_var_ident_from_stmt(stmt: &Stmt, fixed_param_count: usize) -> Option<Ident> {
    detect_copy_var_ident_from_stmt_with_mark(stmt, fixed_param_count, None)
}

fn detect_copy_var_ident_from_stmt_with_mark(
    stmt: &Stmt,
    fixed_param_count: usize,
    unresolved_mark: Option<Mark>,
) -> Option<Ident> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        return None;
    };
    if init.decls.len() != 3 {
        return None;
    }

    // Decl 0: len = arguments.length
    let d0 = &init.decls[0];
    let Pat::Ident(BindingIdent { id: len_id, .. }) = &d0.name else {
        return None;
    };
    let Expr::Member(m) = d0.init.as_deref()? else {
        return None;
    };
    let Expr::Ident(src) = m.obj.as_ref() else {
        return None;
    };
    if src.sym != "arguments" {
        return None;
    }
    if !matches!(&m.prop, MemberProp::Ident(p) if p.sym == "length") {
        return None;
    }
    let len = binding_id(len_id);

    // Decl 1: copy = Array(len) or new Array(len)
    let d1 = &init.decls[1];
    let Pat::Ident(BindingIdent { id: copy_id, .. }) = &d1.name else {
        return None;
    };

    let is_array_ctor = |ident: &Ident| {
        ident.sym == "Array" && unresolved_mark.is_none_or(|mark| ident.ctxt.outer() == mark)
    };
    let one_len_arg = |args: &[swc_core::ecma::ast::ExprOrSpread]| -> bool {
        args.len() == 1
            && args[0].spread.is_none()
            && is_copy_array_len_expr(args[0].expr.as_ref(), &len, fixed_param_count)
    };

    match d1.init.as_deref()? {
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(id) = callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(id) || !one_len_arg(&call.args) {
                return None;
            }
        }
        Expr::New(new_expr) => {
            let Expr::Ident(id) = new_expr.callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(id) {
                return None;
            }
            let args = new_expr.args.as_deref().unwrap_or(&[]);
            if !one_len_arg(args) {
                return None;
            }
        }
        _ => return None,
    }

    // Decl 2: idx = 0 for whole-arguments copies, or idx = fixed_param_count
    // for SWC/Babel tail-rest copy loops.
    let d2 = &init.decls[2];
    let Pat::Ident(BindingIdent { id: idx_id, .. }) = &d2.name else {
        return None;
    };
    let idx_init = d2.init.as_deref()?;
    if !is_number(idx_init, fixed_param_count) {
        return None;
    }
    let idx = binding_id(idx_id);
    let copy = binding_id(copy_id);

    if !matches_copy_loop_test(for_stmt.test.as_deref(), &idx, &len)
        || !matches_copy_loop_update(for_stmt.update.as_deref(), &idx)
        || !matches_copy_loop_body(&for_stmt.body, &copy, &idx, fixed_param_count)
    {
        return None;
    }

    Some(copy_id.clone())
}

fn ts_empty_array_ident_from_stmt(stmt: &Stmt) -> Option<Ident> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.decls.len() != 1 {
        return None;
    }

    let decl = &var.decls[0];
    let Pat::Ident(BindingIdent { id, .. }) = &decl.name else {
        return None;
    };
    let Expr::Array(array) = decl.init.as_deref()? else {
        return None;
    };
    array.elems.is_empty().then(|| id.clone())
}

fn detect_ts_copy_loop_from_stmt(stmt: &Stmt, fixed_param_count: usize) -> Option<BindingId> {
    let Stmt::For(for_stmt) = stmt else {
        return None;
    };
    let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        return None;
    };
    if init.decls.len() != 1 {
        return None;
    }

    let decl = &init.decls[0];
    let Pat::Ident(BindingIdent { id: idx_id, .. }) = &decl.name else {
        return None;
    };
    if !matches!(decl.init.as_deref(), Some(expr) if is_number(expr, fixed_param_count)) {
        return None;
    }

    let idx = binding_id(idx_id);
    if !matches_ts_copy_loop_test(for_stmt.test.as_deref(), &idx)
        || !matches_copy_loop_update(for_stmt.update.as_deref(), &idx)
    {
        return None;
    }

    copy_var_from_ts_copy_loop_body(&for_stmt.body, &idx, fixed_param_count)
}

fn matches_ts_copy_loop_test(test: Option<&Expr>, idx: &BindingId) -> bool {
    let Some(Expr::Bin(bin)) = test else {
        return false;
    };
    bin.op == BinaryOp::Lt
        && matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx))
        && is_arguments_length_expr(bin.right.as_ref())
}

fn copy_var_from_ts_copy_loop_body(
    body: &Stmt,
    idx: &BindingId,
    fixed_param_count: usize,
) -> Option<BindingId> {
    let expr = match body {
        Stmt::Expr(expr) => expr.expr.as_ref(),
        Stmt::Block(block) => {
            if block.stmts.len() != 1 {
                return None;
            }
            let Stmt::Expr(expr) = &block.stmts[0] else {
                return None;
            };
            expr.expr.as_ref()
        }
        _ => return None,
    };

    let Expr::Assign(assign) = expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }

    let AssignTarget::Simple(SimpleAssignTarget::Member(left)) = &assign.left else {
        return None;
    };
    let Expr::Ident(copy_id) = left.obj.as_ref() else {
        return None;
    };
    let MemberProp::Computed(left_prop) = &left.prop else {
        return None;
    };
    if !is_copy_write_index(left_prop.expr.as_ref(), idx, fixed_param_count) {
        return None;
    }

    let Expr::Member(right) = assign.right.as_ref() else {
        return None;
    };
    if !is_arguments_ident(&right.obj) {
        return None;
    }
    let MemberProp::Computed(right_prop) = &right.prop else {
        return None;
    };
    if !matches!(right_prop.expr.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx)) {
        return None;
    }

    Some(binding_id(copy_id))
}

fn is_copy_array_len_expr(expr: &Expr, len: &BindingId, fixed_param_count: usize) -> bool {
    if fixed_param_count == 0 {
        return matches!(expr, Expr::Ident(id) if ident_matches_binding(id, len));
    }

    let Expr::Cond(cond) = expr else {
        return false;
    };

    matches!(
        cond.test.as_ref(),
        Expr::Bin(bin)
            if bin.op == BinaryOp::Gt
                && matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, len))
                && is_number(bin.right.as_ref(), fixed_param_count)
    ) && matches!(
        cond.cons.as_ref(),
        Expr::Bin(bin)
            if bin.op == BinaryOp::Sub
                && matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, len))
                && is_number(bin.right.as_ref(), fixed_param_count)
    ) && is_number(cond.alt.as_ref(), 0)
}

fn is_number(expr: &Expr, expected: usize) -> bool {
    matches!(expr, Expr::Lit(Lit::Num(number)) if number.value == expected as f64)
}

fn matches_copy_loop_test(test: Option<&Expr>, idx: &BindingId, len: &BindingId) -> bool {
    let Some(Expr::Bin(bin)) = test else {
        return false;
    };
    bin.op == BinaryOp::Lt
        && matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx))
        && matches!(bin.right.as_ref(), Expr::Ident(id) if ident_matches_binding(id, len))
}

fn matches_copy_loop_update(update: Option<&Expr>, idx: &BindingId) -> bool {
    let Some(Expr::Update(update)) = update else {
        return false;
    };
    update.op == UpdateOp::PlusPlus
        && matches!(update.arg.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx))
}

fn matches_copy_loop_body(
    body: &Stmt,
    copy: &BindingId,
    idx: &BindingId,
    fixed_param_count: usize,
) -> bool {
    let expr = match body {
        Stmt::Expr(expr) => expr.expr.as_ref(),
        Stmt::Block(block) => {
            if block.stmts.len() != 1 {
                return false;
            }
            let Stmt::Expr(expr) = &block.stmts[0] else {
                return false;
            };
            expr.expr.as_ref()
        }
        _ => return false,
    };

    let Expr::Assign(assign) = expr else {
        return false;
    };
    if assign.op != AssignOp::Assign {
        return false;
    }

    let AssignTarget::Simple(SimpleAssignTarget::Member(left)) = &assign.left else {
        return false;
    };
    if !matches!(left.obj.as_ref(), Expr::Ident(id) if ident_matches_binding(id, copy)) {
        return false;
    }
    let MemberProp::Computed(left_prop) = &left.prop else {
        return false;
    };
    if !is_copy_write_index(left_prop.expr.as_ref(), idx, fixed_param_count) {
        return false;
    }

    let Expr::Member(right) = assign.right.as_ref() else {
        return false;
    };
    if !is_arguments_ident(&right.obj) {
        return false;
    }
    let MemberProp::Computed(right_prop) = &right.prop else {
        return false;
    };
    matches!(right_prop.expr.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx))
}

fn is_copy_write_index(expr: &Expr, idx: &BindingId, fixed_param_count: usize) -> bool {
    if fixed_param_count == 0 {
        return matches!(expr, Expr::Ident(id) if ident_matches_binding(id, idx));
    }

    matches!(
        expr,
        Expr::Bin(bin)
            if bin.op == BinaryOp::Sub
                && matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, idx))
                && is_number(bin.right.as_ref(), fixed_param_count)
    )
}

fn make_rest_param(ident: Ident) -> Param {
    Param {
        span: DUMMY_SP,
        decorators: vec![],
        pat: Pat::Rest(RestPat {
            span: DUMMY_SP,
            dot3_token: DUMMY_SP,
            arg: Box::new(Pat::Ident(BindingIdent {
                id: ident,
                type_ann: None,
            })),
            type_ann: None,
        }),
    }
}

// ============================================================
// Visitor: classify all `arguments` usages as safe or unsafe
// ============================================================

#[derive(Default)]
struct ArgumentsChecker {
    has_any: bool,
    has_unsafe: bool,
    fixed_param_count: usize,
    allowed_loop_indices: Vec<BindingId>,
    arguments_length_aliases: Vec<BindingId>,
    zero_initialized_indices: Vec<BindingId>,
}

impl ArgumentsChecker {
    fn new(fixed_param_count: usize) -> Self {
        Self {
            has_any: false,
            has_unsafe: false,
            fixed_param_count,
            allowed_loop_indices: Vec::new(),
            arguments_length_aliases: Vec::new(),
            zero_initialized_indices: Vec::new(),
        }
    }

    fn is_safe_arguments_index(&self, expr: &Expr) -> bool {
        if self.fixed_param_count == 0 {
            return extract_numeric_index(expr).is_some()
                || matches!(
                    expr,
                    Expr::Ident(id)
                        if self
                            .allowed_loop_indices
                            .iter()
                            .any(|index| ident_matches_binding(id, index))
                )
                || matches!(
                    expr,
                    Expr::Update(update)
                        if update.op == UpdateOp::PlusPlus
                            && matches!(
                                update.arg.as_ref(),
                                Expr::Ident(id)
                                    if self
                                        .allowed_loop_indices
                                        .iter()
                                        .any(|index| ident_matches_binding(id, index))
                            )
                );
        }

        is_safe_arguments_index(expr, self.fixed_param_count)
    }
}

impl Visit for ArgumentsChecker {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Some(alias) = arguments_length_alias_from_stmt(stmt) {
            self.arguments_length_aliases.push(alias);
        }
        self.zero_initialized_indices
            .extend(zero_initialized_bindings_from_stmt(stmt));

        if detect_copy_var_ident_from_stmt(stmt, self.fixed_param_count).is_some() {
            self.has_any = true;
            return;
        }
        if detect_ts_copy_loop_from_stmt(stmt, self.fixed_param_count).is_some() {
            self.has_any = true;
            return;
        }

        if let Some(index_sym) = guarded_arguments_length_loop_index(
            stmt,
            &self.arguments_length_aliases,
            &self.zero_initialized_indices,
        ) {
            self.allowed_loop_indices.push(index_sym);
            stmt.visit_children_with(self);
            self.allowed_loop_indices.pop();
            return;
        }

        stmt.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, expr: &MemberExpr) {
        if is_arguments_ident(&expr.obj) {
            self.has_any = true;
            match &expr.prop {
                // arguments[expr] — any subscript access is safe; the rest array
                // supports arbitrary indexing the same way when there are no
                // fixed params. With fixed params, only proven tail indexes are safe.
                MemberProp::Computed(computed) if self.is_safe_arguments_index(&computed.expr) => {}
                // arguments.length — safe only in parameter-less functions
                MemberProp::Ident(i) if i.sym == "length" && self.fixed_param_count == 0 => {}
                // arguments.callee, arguments.anything_else — unsafe
                _ => {
                    self.has_unsafe = true;
                }
            }
            // Don't recurse: we've handled the `arguments` object reference and
            // don't want visit_ident to fire for the inner `arguments` ident.
            return;
        }
        expr.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        // Any bare `arguments` reference that wasn't caught as a safe member
        // access above (e.g. passed as a value, spread, etc.) is unsafe.
        if id.sym == "arguments" {
            self.has_any = true;
            self.has_unsafe = true;
        }
    }

    // Don't descend into nested functions or class constructors — they have
    // their own `arguments`. Arrows have none and read this function's object,
    // so the default traversal covers them.
    fn visit_function(&mut self, _: &Function) {}
    fn visit_constructor(&mut self, _: &Constructor) {}
}

/// True when any node in `node` mentions `arguments` from the enclosing
/// function's activation: nested arrows count, nested functions do not.
fn mentions_arguments<N: VisitWith<ArgumentsMentionDetector>>(node: &N) -> bool {
    let mut detector = ArgumentsMentionDetector { found: false };
    node.visit_with(&mut detector);
    detector.found
}

struct ArgumentsMentionDetector {
    found: bool,
}

impl Visit for ArgumentsMentionDetector {
    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == "arguments" {
            self.found = true;
        }
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_constructor(&mut self, _: &Constructor) {}
}

/// Reuse a copy binding's name and context when possible. If another binding
/// has the same printed name, rename the copy and its references before
/// removing its loop. The nested binding itself must not be renamed.
fn prepare_rest_ident<P: VisitWith<IdentNameCollector>>(
    body: &mut FunctionBody,
    params: &P,
    copy: Option<&Ident>,
) -> Ident {
    let preferred = copy.map_or_else(|| Atom::from("args"), |ident| ident.sym.clone());
    let name = fresh_rest_name(body, params, preferred, copy.map(binding_id));
    let Some(copy) = copy else {
        return fresh_binding_ident(name, DUMMY_SP);
    };
    if name != copy.sym {
        rename_bindings(
            body,
            &[BindingRename {
                old: binding_id(copy),
                new: name.clone(),
            }],
        );
    }
    let mut rest = copy.clone();
    rest.sym = name;
    rest
}

/// Printed JavaScript has no SyntaxContext. Reserve every identifier spelling
/// except the copy binding being replaced, then pick the preferred name or an
/// unused suffix. This covers both capture of inserted reads and outer names.
fn fresh_rest_name<P: VisitWith<IdentNameCollector>>(
    body: &FunctionBody,
    params: &P,
    preferred: Atom,
    copy: Option<BindingId>,
) -> Atom {
    let mut collector = IdentNameCollector {
        names: HashSet::default(),
        ignored_binding: copy,
    };
    body.visit_with(&mut collector);
    collector.ignored_binding = None;
    params.visit_with(&mut collector);

    if !collector.names.contains(&preferred) {
        return preferred;
    }
    (1usize..)
        .map(|suffix| Atom::from(format!("{preferred}_{suffix}")))
        .find(|candidate| !collector.names.contains(candidate))
        .expect("an unused suffix exists")
}

struct IdentNameCollector {
    names: HashSet<Atom>,
    ignored_binding: Option<BindingId>,
}

impl Visit for IdentNameCollector {
    fn visit_ident(&mut self, ident: &Ident) {
        if self
            .ignored_binding
            .as_ref()
            .is_some_and(|binding| ident_matches_binding(ident, binding))
        {
            return;
        }
        self.names.insert(ident.sym.clone());
    }
}

fn is_arguments_ident(expr: &Expr) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == "arguments")
}

fn arguments_length_alias_from_stmt(stmt: &Stmt) -> Option<BindingId> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return None;
    };
    if var.kind != VarDeclKind::Const || var.decls.len() != 1 {
        return None;
    }

    let decl = &var.decls[0];
    let Pat::Ident(binding) = &decl.name else {
        return None;
    };
    let Expr::Member(member) = decl.init.as_deref()? else {
        return None;
    };
    if is_arguments_ident(&member.obj)
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "length")
    {
        return Some(binding_id(&binding.id));
    }

    None
}

fn zero_initialized_bindings_from_stmt(stmt: &Stmt) -> Vec<BindingId> {
    let Stmt::Decl(Decl::Var(var)) = stmt else {
        return Vec::new();
    };

    var.decls
        .iter()
        .filter_map(|decl| {
            let Pat::Ident(binding) = &decl.name else {
                return None;
            };
            if matches!(decl.init.as_deref(), Some(expr) if is_number(expr, 0)) {
                return Some(binding_id(&binding.id));
            }
            None
        })
        .collect()
}

fn guarded_arguments_length_loop_index(
    stmt: &Stmt,
    length_aliases: &[BindingId],
    zero_initialized_indices: &[BindingId],
) -> Option<BindingId> {
    match stmt {
        Stmt::For(for_stmt) => arguments_length_for_loop_index(for_stmt, length_aliases),
        Stmt::While(while_stmt) => {
            arguments_length_while_loop_index(while_stmt, length_aliases, zero_initialized_indices)
        }
        _ => None,
    }
}

fn arguments_length_for_loop_index(
    for_stmt: &swc_core::ecma::ast::ForStmt,
    length_aliases: &[BindingId],
) -> Option<BindingId> {
    let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        return None;
    };

    let mut local_length_aliases = length_aliases.to_vec();
    let mut index_candidates = Vec::new();
    for decl in &init.decls {
        let Pat::Ident(binding) = &decl.name else {
            continue;
        };
        let binding = binding_id(&binding.id);
        match decl.init.as_deref() {
            Some(expr) if is_number(expr, 0) => index_candidates.push(binding),
            Some(expr) if is_arguments_length_expr(expr) => local_length_aliases.push(binding),
            _ => {}
        }
    }

    index_candidates.into_iter().find(|index| {
        matches_arguments_length_loop_test(for_stmt.test.as_deref(), index, &local_length_aliases)
            && (for_stmt.update.is_none()
                || matches_loop_index_update(for_stmt.update.as_deref(), index))
    })
}

fn arguments_length_while_loop_index(
    while_stmt: &swc_core::ecma::ast::WhileStmt,
    length_aliases: &[BindingId],
    zero_initialized_indices: &[BindingId],
) -> Option<BindingId> {
    zero_initialized_indices.iter().find_map(|index| {
        matches_arguments_length_loop_test(Some(while_stmt.test.as_ref()), index, length_aliases)
            .then_some(index.clone())
    })
}

fn matches_arguments_length_loop_test(
    test: Option<&Expr>,
    index: &BindingId,
    length_aliases: &[BindingId],
) -> bool {
    let Some(Expr::Bin(bin)) = test else {
        return false;
    };
    if bin.op != BinaryOp::Lt {
        return false;
    }
    if !matches!(bin.left.as_ref(), Expr::Ident(id) if ident_matches_binding(id, index)) {
        return false;
    }
    is_arguments_length_expr(bin.right.as_ref())
        || is_binding_ident(bin.right.as_ref(), length_aliases)
}

fn matches_loop_index_update(update: Option<&Expr>, index: &BindingId) -> bool {
    let Some(Expr::Update(update)) = update else {
        return false;
    };
    update.op == UpdateOp::PlusPlus
        && matches!(update.arg.as_ref(), Expr::Ident(id) if ident_matches_binding(id, index))
}

fn is_arguments_length_expr(expr: &Expr) -> bool {
    let Expr::Member(member) = expr else {
        return false;
    };
    is_arguments_ident(&member.obj)
        && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "length")
}

fn is_binding_ident(expr: &Expr, bindings: &[BindingId]) -> bool {
    matches!(expr, Expr::Ident(id) if bindings.iter().any(|binding| ident_matches_binding(id, binding)))
}

// ============================================================
// VisitMut: rewrite `arguments` → rest param name in member exprs
// ============================================================

/// Remove the Babel arguments copy loop:
/// `for (var len = arguments.length, arr = Array(len), i = 0; i < len; i++) arr[i] = arguments[i];`
/// This loop is dead code once the rest param is added.
fn remove_arguments_copy_loop(body: &mut FunctionBody, fixed_param_count: usize) {
    let old = std::mem::take(&mut body.stmts);
    let mut result = Vec::with_capacity(old.len());
    let mut i = 0;
    // Where the removed loop stood, and the bindings its head declared besides
    // the copy: the length alias and the index. Minified code reuses them
    // after the loop (`e` becomes the `_this` alias), so they are re-declared
    // there when a reference survives.
    let mut removed_loop: Option<(usize, Vec<(Ident, Box<Expr>)>)> = None;

    while i < old.len() {
        if let Some(copy) = detect_copy_var_ident_from_stmt(&old[i], fixed_param_count) {
            if removed_loop.is_none() {
                removed_loop = Some((
                    result.len(),
                    copy_loop_side_bindings(&old[i], &binding_id(&copy)),
                ));
            }
            i += 1;
            continue;
        }

        if i + 1 < old.len() {
            if let Some(copy) = ts_empty_array_ident_from_stmt(&old[i]) {
                if detect_ts_copy_loop_from_stmt(&old[i + 1], fixed_param_count)
                    .is_some_and(|loop_copy| loop_copy == binding_id(&copy))
                {
                    if removed_loop.is_none() {
                        removed_loop = Some((
                            result.len(),
                            copy_loop_side_bindings(&old[i + 1], &binding_id(&copy)),
                        ));
                    }
                    i += 2;
                    continue;
                }
            }
        }

        if let Some(copy) = detect_ts_copy_loop_from_stmt(&old[i], fixed_param_count) {
            if removed_loop.is_none() {
                removed_loop = Some((result.len(), copy_loop_side_bindings(&old[i], &copy)));
            }
            i += 1;
            continue;
        }

        result.push(old[i].clone());
        i += 1;
    }

    if let Some((index, bindings)) = removed_loop {
        let decls: Vec<VarDeclarator> = bindings
            .into_iter()
            .filter(|(ident, _)| {
                let key = binding_id(ident);
                result.iter().any(|stmt| count_binding_refs(stmt, &key) > 0)
            })
            .map(|(ident, length)| VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(BindingIdent {
                    id: ident,
                    type_ann: None,
                }),
                init: Some(length),
                definite: false,
            })
            .collect();
        if !decls.is_empty() {
            result.insert(
                index,
                Stmt::Decl(Decl::Var(Box::new(VarDecl {
                    span: DUMMY_SP,
                    ctxt: Default::default(),
                    kind: VarDeclKind::Var,
                    declare: false,
                    decls,
                }))),
            );
        }
    }

    body.stmts = result;
}

/// The `var` bindings a copy loop's head declares other than the copy itself,
/// each paired with its terminal value. The length alias keeps
/// `arguments.length`, while the index cannot finish below its initial value
/// when fewer arguments than fixed parameters were supplied. Clone expressions
/// so references retain their resolver contexts.
fn copy_loop_side_bindings(stmt: &Stmt, copy: &BindingId) -> Vec<(Ident, Box<Expr>)> {
    let Stmt::For(for_stmt) = stmt else {
        return Vec::new();
    };
    let Some(VarDeclOrExpr::VarDecl(init)) = &for_stmt.init else {
        return Vec::new();
    };
    let from_head = init
        .decls
        .first()
        .and_then(|decl| decl.init.as_deref())
        .filter(|expr| is_arguments_length_expr(expr));
    let from_test = for_stmt.test.as_deref().and_then(|test| match test {
        Expr::Bin(bin) if is_arguments_length_expr(&bin.right) => Some(bin.right.as_ref()),
        _ => None,
    });
    let Some(length) = from_head.or(from_test) else {
        return Vec::new();
    };
    init.decls
        .iter()
        .filter_map(|decl| match &decl.name {
            Pat::Ident(binding) if binding_id(&binding.id) != *copy => {
                let value = match decl.init.as_deref() {
                    Some(start @ Expr::Lit(Lit::Num(number))) if number.value > 0.0 => {
                        Expr::Cond(CondExpr {
                            span: DUMMY_SP,
                            test: Box::new(Expr::Bin(BinExpr {
                                span: DUMMY_SP,
                                op: BinaryOp::Lt,
                                left: Box::new(length.clone()),
                                right: Box::new(start.clone()),
                            })),
                            cons: Box::new(start.clone()),
                            alt: Box::new(length.clone()),
                        })
                    }
                    _ => length.clone(),
                };
                Some((binding.id.clone(), Box::new(value)))
            }
            _ => None,
        })
        .collect()
}

struct ArgumentsRewriter {
    ident: Ident,
    fixed_param_count: usize,
}

impl VisitMut for ArgumentsRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if is_arguments_ident(&member.obj) {
                if self.fixed_param_count == 0 {
                    *member.obj = Expr::Ident(self.ident.clone());
                    return;
                }

                if let MemberProp::Computed(computed) = &mut member.prop {
                    if let Some(rewritten_index) =
                        rewrite_arguments_index(computed.expr.as_ref(), self.fixed_param_count)
                    {
                        *expr = Expr::Member(MemberExpr {
                            span: member.span,
                            obj: Box::new(Expr::Ident(self.ident.clone())),
                            prop: MemberProp::Computed(swc_core::ecma::ast::ComputedPropName {
                                span: computed.span,
                                expr: Box::new(rewritten_index),
                            }),
                        });
                        return;
                    }
                }

                // Don't recurse — we've already handled or intentionally left this node
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    // Don't descend into nested functions or class constructors; arrows read
    // this function's `arguments` and are rewritten along with the body.
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_constructor(&mut self, _: &mut Constructor) {}
}

fn is_safe_arguments_index(expr: &Expr, fixed_param_count: usize) -> bool {
    if fixed_param_count == 0 {
        return true;
    }

    let Some(index) = extract_numeric_index(expr) else {
        return false;
    };
    index >= fixed_param_count
}

fn rewrite_arguments_index(expr: &Expr, fixed_param_count: usize) -> Option<Expr> {
    if fixed_param_count == 0 {
        return Some(expr.clone());
    }

    let index = extract_numeric_index(expr)?;
    if index < fixed_param_count {
        return None;
    }

    Some(Expr::Lit(Lit::Num(Number {
        span: DUMMY_SP,
        value: (index - fixed_param_count) as f64,
        raw: None,
    })))
}

fn extract_numeric_index(expr: &Expr) -> Option<usize> {
    let Expr::Lit(Lit::Num(number)) = expr else {
        return None;
    };

    let value = number.value;
    if value.fract() != 0.0 || value.is_sign_negative() {
        return None;
    }

    Some(value as usize)
}
