use swc_core::atoms::Atom;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignPat, AssignTarget, BinExpr, BinaryOp, BindingIdent,
    BlockStmt, BlockStmtOrExpr, Bool, Decl, Expr, Function, Ident, IfStmt, Lit, MemberExpr,
    MemberProp, Number, Param, Pat, SimpleAssignTarget, Stmt, UnaryExpr, UnaryOp, VarDeclKind,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnParameters;

impl VisitMut for UnParameters {
    fn visit_mut_function(&mut self, func: &mut Function) {
        func.visit_mut_children_with(self);
        if let Some(body) = &mut func.body {
            process_function_params(&mut func.params, body);
        }
    }

    fn visit_mut_arrow_expr(&mut self, expr: &mut ArrowExpr) {
        expr.visit_mut_children_with(self);
        if let BlockStmtOrExpr::BlockStmt(body) = &mut *expr.body {
            process_arrow_params(&mut expr.params, body);
        }
    }
}

/// Process Pattern A (TypeScript/Babel simple form) and Pattern B (arguments-based)
/// for regular functions with Vec<Param>.
fn process_function_params(params: &mut Vec<Param>, body: &mut BlockStmt) {
    process_pattern_a_params(params, body);
    process_pattern_b_params(params, body);
    rewrite_inline_arguments_defaults(params, body);
}

/// Process Pattern A for arrow functions with Vec<Pat>.
fn process_arrow_params(params: &mut Vec<Pat>, body: &mut BlockStmt) {
    process_pattern_a_arrow_params(params, body);
}

// ============================================================
// Pattern A: `if (a === void 0) { a = 1; }` → default param
// ============================================================

fn process_pattern_a_params(params: &mut Vec<Param>, body: &mut BlockStmt) {
    let mut to_remove: Vec<usize> = Vec::new();

    // Only scan first 15 statements
    let scan_limit = body.stmts.len().min(15);

    for (stmt_idx, stmt) in body.stmts[..scan_limit].iter().enumerate() {
        let extracted = extract_default_param_from_if(stmt);

        if let Some((param_name, default_val)) = extracted {
            // Find the matching parameter
            if let Some(param_idx) = find_plain_param_idx(params, &param_name) {
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

fn process_pattern_a_arrow_params(params: &mut Vec<Pat>, body: &mut BlockStmt) {
    let mut to_remove: Vec<usize> = Vec::new();

    let scan_limit = body.stmts.len().min(15);

    for (stmt_idx, stmt) in body.stmts[..scan_limit].iter().enumerate() {
        let extracted = extract_default_param_from_if(stmt);

        if let Some((param_name, default_val)) = extracted {
            if let Some(param_idx) = find_plain_pat_idx(params, &param_name) {
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
fn extract_default_param_from_if(stmt: &Stmt) -> Option<(Atom, Box<Expr>)> {
    let Stmt::If(IfStmt {
        test, cons, alt, ..
    }) = stmt
    else {
        return None;
    };
    if alt.is_some() {
        return None;
    }

    let param_name = extract_void0_check(test)?;
    let default_val = extract_assign_from_cons(cons, &param_name)?;

    Some((param_name, default_val))
}

/// Check if `expr` is `ident === void 0` or `void 0 === ident`
/// or `ident === undefined` or `undefined === ident`.
/// Returns the identifier name if matched.
fn extract_void0_check(expr: &Expr) -> Option<Atom> {
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
    if is_void0_or_undefined(right) {
        if let Expr::Ident(id) = left.as_ref() {
            return Some(id.sym.clone());
        }
    }
    // void 0 === right / undefined === right
    if is_void0_or_undefined(left) {
        if let Expr::Ident(id) = right.as_ref() {
            return Some(id.sym.clone());
        }
    }

    None
}

fn is_void0_or_undefined(expr: &Expr) -> bool {
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
        if id.sym == "undefined" {
            return true;
        }
    }
    false
}

/// Extract the assigned default value from the consequent branch.
/// Consequent can be:
/// - `ExprStmt(Assign { left: ident, op: =, right: val })`
/// - `Block([ExprStmt(Assign { left: ident, op: =, right: val })])`
fn extract_assign_from_cons(cons: &Stmt, param_name: &Atom) -> Option<Box<Expr>> {
    match cons {
        Stmt::Expr(expr_stmt) => extract_assign_expr(&expr_stmt.expr, param_name),
        Stmt::Block(block) => {
            if block.stmts.len() != 1 {
                return None;
            }
            let Stmt::Expr(expr_stmt) = &block.stmts[0] else {
                return None;
            };
            extract_assign_expr(&expr_stmt.expr, param_name)
        }
        _ => None,
    }
}

fn extract_assign_expr(expr: &Expr, param_name: &Atom) -> Option<Box<Expr>> {
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
    if &ident.id.sym != param_name {
        return None;
    }
    Some(right.clone())
}

// ============================================================
// Pattern B: arguments-based default params
// ============================================================

fn process_pattern_b_params(params: &mut Vec<Param>, body: &mut BlockStmt) {
    let mut to_remove: Vec<usize> = Vec::new();

    // Scan entire body for var declarations matching the arguments pattern
    for (stmt_idx, stmt) in body.stmts.iter().enumerate() {
        if let Some((param_idx, param_name, default_val)) = extract_arguments_default(stmt) {
            if ensure_default_param(params, param_idx, param_name, default_val).is_some() {
                to_remove.push(stmt_idx);
            }
            continue;
        }

        if let Some((param_idx, param_name)) = extract_arguments_alias(stmt) {
            if ensure_plain_param(params, param_idx, param_name).is_some() {
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
fn extract_arguments_default(stmt: &Stmt) -> Option<(usize, Atom, Box<Expr>)> {
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

    let (param_idx, default_val) = extract_arguments_default_expr(init.as_ref())?;
    Some((param_idx, var_ident.sym.clone(), default_val))
}

/// Check if `expr` is `arguments.length > N ? arguments[N] : undefined` (simple optional)
fn extract_simple_arguments_optional(expr: &Expr) -> Option<usize> {
    let Expr::Cond(cond) = expr else {
        return None;
    };
    let n = extract_arguments_length_threshold(cond.test.as_ref())?;
    if !is_arguments_idx(&cond.cons, n) {
        return None;
    }
    if !is_void0_or_undefined(&cond.alt) {
        return None;
    }
    Some(n)
}

/// Extract index N from `arguments.length > N && arguments[N] !== undefined`
fn extract_arguments_cond_index(expr: &Expr) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };

    // Handle: `arguments.length > N && arguments[N] !== undefined`
    if *op == BinaryOp::LogicalAnd {
        let n = extract_arguments_length_threshold(left.as_ref())?;

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
        let args_side = if is_void0_or_undefined(rr) {
            rl.as_ref()
        } else if is_void0_or_undefined(rl) {
            rr.as_ref()
        } else {
            return None;
        };
        if !is_arguments_idx(args_side, n) {
            return None;
        }
        return Some(n);
    }

    None
}

fn extract_arguments_length_threshold(expr: &Expr) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    match *op {
        BinaryOp::Gt if is_arguments_length(left.as_ref()) => extract_num_literal(right.as_ref()),
        BinaryOp::Lt if is_arguments_length(right.as_ref()) => extract_num_literal(left.as_ref()),
        _ => None,
    }
}

fn extract_arguments_default_expr(expr: &Expr) -> Option<(usize, Box<Expr>)> {
    let expr = strip_parens(expr);
    if let Some(n) = extract_simple_arguments_optional(expr) {
        let _ = n;
        return None;
    }

    match expr {
        Expr::Cond(cond) => {
            let param_idx = extract_arguments_cond_index(cond.test.as_ref())?;
            if !is_arguments_idx(cond.cons.as_ref(), param_idx) {
                return None;
            }
            if is_void0_or_undefined(cond.alt.as_ref()) {
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
            let param_idx = extract_arguments_cond_index(arg.as_ref())?;
            if !is_arguments_idx(right.as_ref(), param_idx) {
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

fn extract_arguments_alias(stmt: &Stmt) -> Option<(usize, Atom)> {
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
    let param_idx = extract_arguments_index_expr(init.as_ref())?;
    Some((param_idx, var_ident.sym.clone()))
}

fn extract_arguments_index_expr(expr: &Expr) -> Option<usize> {
    let expr = strip_parens(expr);
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return None;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if id.sym == "arguments") {
        return None;
    }
    let MemberProp::Computed(computed) = prop else {
        return None;
    };
    extract_num_literal(computed.expr.as_ref())
}

/// Check if expr is `arguments.length`
fn is_arguments_length(expr: &Expr) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if id.sym == "arguments") {
        return false;
    }
    matches!(prop, MemberProp::Ident(i) if i.sym == "length")
}

/// Check if expr is `arguments[N]`
fn is_arguments_idx(expr: &Expr, n: usize) -> bool {
    let Expr::Member(MemberExpr { obj, prop, .. }) = expr else {
        return false;
    };
    if !matches!(obj.as_ref(), Expr::Ident(id) if id.sym == "arguments") {
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

fn find_plain_param_idx(params: &[Param], name: &Atom) -> Option<usize> {
    params.iter().position(|p| {
        if let Pat::Ident(BindingIdent { id, .. }) = &p.pat {
            &id.sym == name
        } else {
            false
        }
    })
}

fn find_plain_pat_idx(params: &[Pat], name: &Atom) -> Option<usize> {
    params.iter().position(|p| {
        if let Pat::Ident(BindingIdent { id, .. }) = p {
            &id.sym == name
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

fn strip_parens<'a>(expr: &'a Expr) -> &'a Expr {
    let mut current = expr;
    while let Expr::Paren(paren) = current {
        current = paren.expr.as_ref();
    }
    current
}

fn rewrite_inline_arguments_defaults(params: &mut Vec<Param>, body: &mut BlockStmt) {
    let mut rewriter = InlineArgumentsDefaultRewriter { params };
    body.visit_mut_with(&mut rewriter);
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

fn ensure_plain_param(params: &mut Vec<Param>, idx: usize, preferred_name: Atom) -> Option<Ident> {
    ensure_params_len(params, idx);
    let Pat::Ident(binding) = &mut params[idx].pat else {
        return None;
    };
    if is_placeholder(&binding.id.sym, idx) {
        binding.id.sym = preferred_name;
    }
    Some(binding.id.clone())
}

fn ensure_default_param(
    params: &mut Vec<Param>,
    idx: usize,
    preferred_name: Atom,
    default_val: Box<Expr>,
) -> Option<Ident> {
    ensure_params_len(params, idx);

    let (ident, can_replace) = match &params[idx].pat {
        Pat::Ident(binding) => {
            let mut id = binding.id.clone();
            if is_placeholder(&id.sym, idx) {
                id.sym = preferred_name;
            }
            (id, true)
        }
        _ => (Ident::new_no_ctxt(preferred_name, DUMMY_SP), false),
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

struct InlineArgumentsDefaultRewriter<'a> {
    params: &'a mut Vec<Param>,
}

impl VisitMut for InlineArgumentsDefaultRewriter<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Some((idx, default_val)) = extract_arguments_default_expr(expr) else {
            return;
        };

        let preferred_name = placeholder_name(idx);
        if let Some(ident) = ensure_default_param(self.params, idx, preferred_name, default_val) {
            *expr = Expr::Ident(ident);
        }
    }

    fn visit_mut_function(&mut self, _: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
}
