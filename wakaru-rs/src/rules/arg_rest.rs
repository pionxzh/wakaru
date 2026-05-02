use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignOp, AssignTarget, BinaryOp, BindingIdent, BlockStmt, Callee, Constructor,
    Expr, Function, Ident, Lit, MemberExpr, MemberProp, Number, Param, ParamOrTsParamProp, Pat,
    RestPat, SimpleAssignTarget, Stmt, UpdateOp, VarDeclOrExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

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
        let fixed_param_count = func.params.len();

        let copy_var = detect_copy_var_name(body, fixed_param_count);
        let mut checker = ArgumentsChecker::new(fixed_param_count);
        body.visit_with(&mut checker);

        if !checker.has_any || checker.has_unsafe {
            return;
        }

        // Use the copy variable's name when a Babel copy loop is present; it is
        // already unique within the scope and avoids any naming conflict.
        let rest_name: Atom = copy_var.clone().unwrap_or_else(|| "args".into());
        func.params.push(make_rest_param(rest_name.clone()));

        // Rewrite `arguments` → `args` in the body
        if let Some(body) = &mut func.body {
            // Remove the Babel copy loop since the rest param replaces it
            if copy_var.is_some() {
                remove_arguments_copy_loop(body, fixed_param_count);
            }
            body.visit_mut_with(&mut ArgumentsRewriter {
                name: rest_name,
                fixed_param_count,
            });
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
        let fixed_param_count = ctor.params.len();

        let copy_var = detect_copy_var_name(body, fixed_param_count);
        let mut checker = ArgumentsChecker::new(fixed_param_count);
        body.visit_with(&mut checker);

        if !checker.has_any || checker.has_unsafe {
            return;
        }

        let rest_name: Atom = copy_var.clone().unwrap_or_else(|| "args".into());
        ctor.params.push(ParamOrTsParamProp::Param(make_rest_param(rest_name.clone())));

        if let Some(body) = &mut ctor.body {
            // Remove the Babel copy loop since the rest param replaces it
            if copy_var.is_some() {
                remove_arguments_copy_loop(body, fixed_param_count);
            }
            body.visit_mut_with(&mut ArgumentsRewriter {
                name: rest_name,
                fixed_param_count,
            });
        }
    }
}

/// Scan `body` for the Babel rest-args copy pattern **before** `arguments` is rewritten.
/// Returns the copy variable's name (e.g. `i`, `r`, `t`) so it can be reused as the
/// rest param — this avoids any naming conflicts because minified copy vars are already
/// unique within their enclosing scope.
///
/// Pattern matched (3-declarator for-init, `arguments.length` as source):
/// ```text
/// for (var len = arguments.length, copy = Array(len), idx = 0; …) …
/// ```
fn detect_copy_var_name(body: &BlockStmt, fixed_param_count: usize) -> Option<Atom> {
    body.stmts
        .iter()
        .find_map(|stmt| detect_copy_var_name_from_stmt(stmt, fixed_param_count))
}

fn detect_copy_var_name_from_stmt(stmt: &Stmt, fixed_param_count: usize) -> Option<Atom> {
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
    let len_sym = len_id.sym.clone();

    // Decl 1: copy = Array(len) or new Array(len)
    let d1 = &init.decls[1];
    let Pat::Ident(BindingIdent { id: copy_id, .. }) = &d1.name else {
        return None;
    };

    let is_array_ctor = |sym: &Atom| sym == "Array";
    let one_len_arg = |args: &[swc_core::ecma::ast::ExprOrSpread]| -> bool {
        args.len() == 1
            && args[0].spread.is_none()
            && is_copy_array_len_expr(args[0].expr.as_ref(), &len_sym, fixed_param_count)
    };

    match d1.init.as_deref()? {
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(id) = callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(&id.sym) || !one_len_arg(&call.args) {
                return None;
            }
        }
        Expr::New(new_expr) => {
            let Expr::Ident(id) = new_expr.callee.as_ref() else {
                return None;
            };
            if !is_array_ctor(&id.sym) {
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
    let Some(idx_init) = d2.init.as_deref() else {
        return None;
    };
    if !is_number(idx_init, fixed_param_count) {
        return None;
    }
    let idx_sym = idx_id.sym.clone();
    let copy_sym = copy_id.sym.clone();

    if !matches_copy_loop_test(for_stmt.test.as_deref(), &idx_sym, &len_sym)
        || !matches_copy_loop_update(for_stmt.update.as_deref(), &idx_sym)
        || !matches_copy_loop_body(&for_stmt.body, &copy_sym, &idx_sym, fixed_param_count)
    {
        return None;
    }

    Some(copy_sym)
}

fn is_copy_array_len_expr(expr: &Expr, len_sym: &Atom, fixed_param_count: usize) -> bool {
    if fixed_param_count == 0 {
        return matches!(expr, Expr::Ident(id) if id.sym == *len_sym);
    }

    let Expr::Cond(cond) = expr else {
        return false;
    };

    matches!(
        cond.test.as_ref(),
        Expr::Bin(bin)
            if bin.op == BinaryOp::Gt
                && matches!(bin.left.as_ref(), Expr::Ident(id) if id.sym == *len_sym)
                && is_number(bin.right.as_ref(), fixed_param_count)
    ) && matches!(
        cond.cons.as_ref(),
        Expr::Bin(bin)
            if bin.op == BinaryOp::Sub
                && matches!(bin.left.as_ref(), Expr::Ident(id) if id.sym == *len_sym)
                && is_number(bin.right.as_ref(), fixed_param_count)
    ) && is_number(cond.alt.as_ref(), 0)
}

fn is_number(expr: &Expr, expected: usize) -> bool {
    matches!(expr, Expr::Lit(Lit::Num(number)) if number.value == expected as f64)
}

fn matches_copy_loop_test(test: Option<&Expr>, idx_sym: &Atom, len_sym: &Atom) -> bool {
    let Some(Expr::Bin(bin)) = test else {
        return false;
    };
    bin.op == BinaryOp::Lt
        && matches!(bin.left.as_ref(), Expr::Ident(id) if id.sym == *idx_sym)
        && matches!(bin.right.as_ref(), Expr::Ident(id) if id.sym == *len_sym)
}

fn matches_copy_loop_update(update: Option<&Expr>, idx_sym: &Atom) -> bool {
    let Some(Expr::Update(update)) = update else {
        return false;
    };
    update.op == UpdateOp::PlusPlus
        && matches!(update.arg.as_ref(), Expr::Ident(id) if id.sym == *idx_sym)
}

fn matches_copy_loop_body(
    body: &Stmt,
    copy_sym: &Atom,
    idx_sym: &Atom,
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
    if !matches!(left.obj.as_ref(), Expr::Ident(id) if id.sym == *copy_sym) {
        return false;
    }
    let MemberProp::Computed(left_prop) = &left.prop else {
        return false;
    };
    if !is_copy_write_index(left_prop.expr.as_ref(), idx_sym, fixed_param_count) {
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
    matches!(right_prop.expr.as_ref(), Expr::Ident(id) if id.sym == *idx_sym)
}

fn is_copy_write_index(expr: &Expr, idx_sym: &Atom, fixed_param_count: usize) -> bool {
    if fixed_param_count == 0 {
        return matches!(expr, Expr::Ident(id) if id.sym == *idx_sym);
    }

    matches!(
        expr,
        Expr::Bin(bin)
            if bin.op == BinaryOp::Sub
                && matches!(bin.left.as_ref(), Expr::Ident(id) if id.sym == *idx_sym)
                && is_number(bin.right.as_ref(), fixed_param_count)
    )
}

fn make_rest_param(name: Atom) -> Param {
    Param {
        span: DUMMY_SP,
        decorators: vec![],
        pat: Pat::Rest(RestPat {
            span: DUMMY_SP,
            dot3_token: DUMMY_SP,
            arg: Box::new(Pat::Ident(BindingIdent {
                id: Ident::new_no_ctxt(name, DUMMY_SP),
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
}

impl ArgumentsChecker {
    fn new(fixed_param_count: usize) -> Self {
        Self {
            has_any: false,
            has_unsafe: false,
            fixed_param_count,
        }
    }
}

impl Visit for ArgumentsChecker {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if detect_copy_var_name_from_stmt(stmt, self.fixed_param_count).is_some() {
            self.has_any = true;
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
                MemberProp::Computed(computed)
                    if is_safe_arguments_index(computed.expr.as_ref(), self.fixed_param_count) => {}
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

    // Don't descend into nested functions — they have their own `arguments`
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
}

fn is_arguments_ident(expr: &Expr) -> bool {
    matches!(expr, Expr::Ident(id) if id.sym == "arguments")
}

// ============================================================
// VisitMut: rewrite `arguments` → rest param name in member exprs
// ============================================================

/// Remove the Babel arguments copy loop:
/// `for (var len = arguments.length, arr = Array(len), i = 0; i < len; i++) arr[i] = arguments[i];`
/// This loop is dead code once the rest param is added.
fn remove_arguments_copy_loop(body: &mut BlockStmt, fixed_param_count: usize) {
    body.stmts.retain(|stmt| {
        detect_copy_var_name_from_stmt(stmt, fixed_param_count).is_none()
    });
}

struct ArgumentsRewriter {
    name: Atom,
    fixed_param_count: usize,
}

impl VisitMut for ArgumentsRewriter {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Member(member) = expr {
            if is_arguments_ident(&member.obj) {
                if self.fixed_param_count == 0 {
                    *member.obj = Expr::Ident(Ident::new_no_ctxt(self.name.clone(), DUMMY_SP));
                    return;
                }

                if let MemberProp::Computed(computed) = &mut member.prop {
                    if let Some(rewritten_index) =
                        rewrite_arguments_index(computed.expr.as_ref(), self.fixed_param_count)
                    {
                        *expr = Expr::Member(MemberExpr {
                            span: member.span,
                            obj: Box::new(Expr::Ident(Ident::new_no_ctxt(
                                self.name.clone(),
                                DUMMY_SP,
                            ))),
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

    // Don't descend into nested functions
    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
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
