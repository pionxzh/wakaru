use std::collections::{HashMap, HashSet};

use swc_core::common::SyntaxContext;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, CondExpr, Expr, Ident, IfStmt,
    Lit, MemberExpr, Module, OptCall, OptChainBase, OptChainExpr, Pat, SimpleAssignTarget, Stmt,
    UnaryExpr, UnaryOp, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::un_nullish_coalescing::{exprs_structurally_equal, is_undefined};
use super::RewriteLevel;

pub struct UnOptionalChaining {
    level: RewriteLevel,
    uninitialized_bindings: HashSet<BindingId>,
    binding_references: HashMap<BindingId, usize>,
}

impl UnOptionalChaining {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            uninitialized_bindings: HashSet::new(),
            binding_references: HashMap::new(),
        }
    }
}

impl VisitMut for UnOptionalChaining {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let facts = collect_temp_binding_facts(module);
        self.uninitialized_bindings = facts.uninitialized;
        self.binding_references = facts.references;
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(result) = try_optional_call_cleanup(expr) {
            *expr = result;
            return;
        }

        if let Some(result) = try_optional_chaining(
            expr,
            self.level,
            &self.uninitialized_bindings,
            &self.binding_references,
        ) {
            *expr = result;
        }
    }

    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        if let Some(result) = try_optional_call_short_circuit_stmt(stmt) {
            *stmt = result;
            return;
        }

        if let Some(result) = try_optional_call_if_stmt(stmt) {
            *stmt = result;
        }
    }
}

type BindingId = (swc_core::atoms::Atom, SyntaxContext);

fn try_optional_call_cleanup(expr: &Expr) -> Option<Expr> {
    let Expr::OptChain(OptChainExpr { base, .. }) = expr else {
        return None;
    };
    let OptChainBase::Call(call) = base.as_ref() else {
        return None;
    };
    let (context, call_args) = call.args.split_first()?;
    let callee = extract_optional_call_target(call.callee.as_ref(), context.expr.as_ref())?;
    Some(Expr::OptChain(OptChainExpr {
        span: DUMMY_SP,
        optional: true,
        base: Box::new(OptChainBase::Call(OptCall {
            span: call.span,
            ctxt: call.ctxt,
            callee: Box::new(callee),
            args: call_args.to_vec(),
            type_args: call.type_args.clone(),
        })),
    }))
}

fn try_optional_call_if_stmt(stmt: &Stmt) -> Option<Stmt> {
    let Stmt::If(IfStmt {
        test,
        cons,
        alt: None,
        ..
    }) = stmt
    else {
        return None;
    };
    let Expr::Unary(UnaryExpr {
        op: UnaryOp::Bang,
        arg,
        ..
    }) = test.as_ref()
    else {
        return None;
    };

    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(strip_parens(arg))?;
    let real_rhs = real_value?;
    let call_expr = extract_single_call_expr(cons)?;
    build_optional_call_stmt(DUMMY_SP, &checked, &real_rhs, call_expr)
}

fn try_optional_call_short_circuit_stmt(stmt: &Stmt) -> Option<Stmt> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = expr_stmt.expr.as_ref()
    else {
        return None;
    };

    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(left)?;
    let real_rhs = real_value?;
    build_optional_call_stmt(expr_stmt.span, &checked, &real_rhs, right.as_ref())
}

fn extract_single_call_expr(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Block(block) if block.stmts.len() == 1 => extract_single_call_expr(&block.stmts[0]),
        Stmt::Expr(expr_stmt) => Some(expr_stmt.expr.as_ref()),
        _ => None,
    }
}

fn build_optional_call_stmt(
    stmt_span: swc_core::common::Span,
    checked: &Expr,
    real_rhs: &Expr,
    call_expr: &Expr,
) -> Option<Stmt> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee_expr),
        args,
        type_args,
        span,
        ctxt,
    }) = call_expr
    else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = callee_expr.as_ref() else {
        return None;
    };
    if !prop.is_ident_with("call") || !exprs_structurally_equal(obj, checked) {
        return None;
    }

    let (context, call_args) = args.split_first()?;
    let callee = match strip_parens(real_rhs) {
        Expr::OptChain(OptChainExpr { base, .. })
            if matches!(base.as_ref(), OptChainBase::Member(_)) =>
        {
            real_rhs.clone()
        }
        _ => recover_babel_optional_call_callee(real_rhs, context.expr.as_ref())?,
    };

    Some(Stmt::Expr(swc_core::ecma::ast::ExprStmt {
        span: stmt_span,
        expr: Box::new(Expr::OptChain(OptChainExpr {
            span: DUMMY_SP,
            optional: true,
            base: Box::new(OptChainBase::Call(OptCall {
                span: *span,
                ctxt: *ctxt,
                callee: Box::new(callee),
                args: call_args.to_vec(),
                type_args: type_args.clone(),
            })),
        })),
    }))
}

fn try_optional_chaining(
    expr: &Expr,
    level: RewriteLevel,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    // Pattern: `(obj === null || obj === void 0) ? void 0 : obj.access`
    if let Some(result) =
        try_ternary_optional_chain(expr, level, uninitialized_bindings, binding_references)
    {
        return Some(result);
    }

    // Pattern: `obj == null ? undefined : obj.access`  (loose equality)
    if let Some(result) =
        try_loose_eq_optional_chain(expr, level, uninitialized_bindings, binding_references)
    {
        return Some(result);
    }

    None
}

/// Handle: `(obj === null || obj === void 0) ? void 0 : obj.access`  →  `obj?.access`
/// Also handles assignment form: `(tmp = expr) === null || tmp === void 0 ? void 0 : tmp.access`
fn try_ternary_optional_chain(
    expr: &Expr,
    level: RewriteLevel,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = expr
    else {
        return None;
    };

    // consequent must be `void 0` or `undefined`
    if !is_void_or_undefined(cons) {
        return None;
    }

    // test must be `x === null || x === void 0`
    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(test)?;

    if let Some(real_rhs) = real_value {
        // Strict Babel lowering references the temp four times:
        // assignment target, null check left/right, and the final access/call.
        if is_standard_temp_expr(&checked, uninitialized_bindings, binding_references, 4)
            // Generated member/nested-chain temps skip the declaration-site proof, so we only
            // require the three references inside the lowered chain itself.
            || is_generated_member_temp_expr(&checked, &real_rhs, binding_references, 3)
            || is_nested_optional_chain_temp_expr(&checked, &real_rhs, binding_references, 3)
        {
            if let Some(chain) = make_optional_chain_replacing(&checked, &real_rhs, alt) {
                return (level >= RewriteLevel::Standard).then_some(chain);
            }
        }
        if level < RewriteLevel::Aggressive {
            return None;
        }
        // Assignment form: `checked` is `tmp`, `real_rhs` is the original expr
        // alt must use `tmp` as the object
        return make_optional_chain_replacing(&checked, &real_rhs, alt);
    }

    // Plain form
    make_optional_chain(*checked, alt)
}

/// Build `base?.prop` or `base?.method(...)` where `access` uses `base` as its object.
fn make_optional_chain(base: Expr, access: &Expr) -> Option<Expr> {
    match access {
        // x.prop → x?.prop
        Expr::Member(MemberExpr { obj, prop, .. }) if exprs_structurally_equal(obj, &base) => {
            Some(Expr::OptChain(OptChainExpr {
                span: DUMMY_SP,
                optional: true,
                base: Box::new(OptChainBase::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(base),
                    prop: prop.clone(),
                })),
            }))
        }

        // x.method(...) → x?.method(...)
        Expr::Call(CallExpr {
            callee: Callee::Expr(callee_expr),
            args,
            type_args,
            span,
            ctxt,
        }) => {
            if let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr {
                if exprs_structurally_equal(obj, &base) {
                    if prop.is_ident_with("call")
                        && optional_call_context_matches_base(&base, args.first()?.expr.as_ref())
                    {
                        return Some(Expr::OptChain(OptChainExpr {
                            span: DUMMY_SP,
                            optional: true,
                            base: Box::new(OptChainBase::Call(OptCall {
                                span: *span,
                                ctxt: *ctxt,
                                callee: Box::new(base),
                                args: args[1..].to_vec(),
                                type_args: type_args.clone(),
                            })),
                        }));
                    }
                    let opt_member = Expr::OptChain(OptChainExpr {
                        span: DUMMY_SP,
                        optional: true,
                        base: Box::new(OptChainBase::Member(MemberExpr {
                            span: DUMMY_SP,
                            obj: Box::new(base),
                            prop: prop.clone(),
                        })),
                    });
                    return Some(Expr::OptChain(OptChainExpr {
                        span: DUMMY_SP,
                        optional: false,
                        base: Box::new(OptChainBase::Call(OptCall {
                            span: *span,
                            ctxt: *ctxt,
                            callee: Box::new(opt_member),
                            args: args.clone(),
                            type_args: type_args.clone(),
                        })),
                    }));
                }
            }
            None
        }

        _ => None,
    }
}

/// Build an optional chain for the assignment temp-var case.
/// `tmp` is the temp variable expr, `real_rhs` is what it was assigned from.
/// `access` should use `tmp` as its object; we replace `tmp` with `real_rhs` in the output.
fn make_optional_chain_replacing(tmp: &Expr, real_rhs: &Expr, access: &Expr) -> Option<Expr> {
    match access {
        Expr::Member(MemberExpr { obj, prop, .. }) if exprs_structurally_equal(obj, tmp) => {
            Some(Expr::OptChain(OptChainExpr {
                span: DUMMY_SP,
                optional: true,
                base: Box::new(OptChainBase::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(real_rhs.clone()),
                    prop: prop.clone(),
                })),
            }))
        }

        Expr::Call(CallExpr {
            callee: Callee::Expr(callee_expr),
            args,
            type_args,
            span,
            ctxt,
        }) => {
            if let Expr::Member(MemberExpr { obj, prop, .. }) = &**callee_expr {
                if exprs_structurally_equal(obj, tmp) {
                    if prop.is_ident_with("call") {
                        return make_optional_call_replacing(
                            real_rhs, args, type_args, *span, *ctxt,
                        );
                    }
                    let opt_member = Expr::OptChain(OptChainExpr {
                        span: DUMMY_SP,
                        optional: true,
                        base: Box::new(OptChainBase::Member(MemberExpr {
                            span: DUMMY_SP,
                            obj: Box::new(real_rhs.clone()),
                            prop: prop.clone(),
                        })),
                    });
                    return Some(Expr::OptChain(OptChainExpr {
                        span: DUMMY_SP,
                        optional: false,
                        base: Box::new(OptChainBase::Call(OptCall {
                            span: *span,
                            ctxt: *ctxt,
                            callee: Box::new(opt_member),
                            args: args.clone(),
                            type_args: type_args.clone(),
                        })),
                    }));
                }
            }
            None
        }

        _ => None,
    }
}

/// Handle loose equality forms:
/// - `obj == null ? undefined : obj.prop`  →  `obj?.prop`
/// - `obj != null ? obj.prop : undefined`  →  `obj?.prop`
/// - `(tmp = expr) == null ? undefined : tmp.prop`  →  `expr?.prop` (aggressive)
/// - `(tmp = expr) != null ? tmp.prop : undefined`  →  `expr?.prop` (aggressive)
///
/// `x == null` matches both `null` and `undefined`, which is exactly what `?.` does.
fn try_loose_eq_optional_chain(
    expr: &Expr,
    level: RewriteLevel,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    if level < RewriteLevel::Standard {
        return None;
    }

    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = expr
    else {
        return None;
    };

    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = &**test
    else {
        return None;
    };

    match op {
        // `x == null ? undefined : x.prop`
        // `(tmp = expr) == null ? undefined : tmp.prop`
        BinaryOp::EqEq => {
            if !is_void_or_undefined(cons) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right)?;
            try_loose_chain_with_assign(
                checked,
                alt,
                level,
                uninitialized_bindings,
                binding_references,
            )
        }
        // `x != null ? x.prop : undefined`
        // `(tmp = expr) != null ? tmp.prop : undefined`
        BinaryOp::NotEq => {
            if !is_void_or_undefined(alt) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right)?;
            try_loose_chain_with_assign(
                checked,
                cons,
                level,
                uninitialized_bindings,
                binding_references,
            )
        }
        _ => None,
    }
}

fn try_loose_chain_with_assign(
    checked: Expr,
    access: &Expr,
    level: RewriteLevel,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    if let Some((tmp_sym, real_rhs)) = extract_assign_parts(&checked) {
        let tmp_ident_expr = find_ident_by_sym(access, &tmp_sym)?;
        // Loose Babel lowering references the temp twice inside the chain:
        // once in the null check and once in the final access/call.
        if is_standard_temp_expr(
            &tmp_ident_expr,
            uninitialized_bindings,
            binding_references,
            2,
        ) || is_generated_member_temp_expr(&tmp_ident_expr, real_rhs, binding_references, 2)
            || is_nested_optional_chain_temp_expr(&tmp_ident_expr, real_rhs, binding_references, 2)
        {
            if let Some(chain) = make_optional_chain_replacing(&tmp_ident_expr, real_rhs, access) {
                return Some(chain);
            }
        }
        if level < RewriteLevel::Aggressive {
            return None;
        }
        make_optional_chain_replacing(&tmp_ident_expr, real_rhs, access)
    } else {
        make_optional_chain(checked, access)
    }
}

fn find_ident_by_sym(access: &Expr, sym: &swc_core::atoms::Atom) -> Option<Expr> {
    match access {
        Expr::Member(MemberExpr { obj, .. }) => {
            if let Expr::Ident(id) = &**obj {
                if id.sym == *sym {
                    return Some(Expr::Ident(id.clone()));
                }
            }
            None
        }
        Expr::Call(CallExpr {
            callee: Callee::Expr(callee_expr),
            ..
        }) => {
            if let Expr::Member(MemberExpr { obj, .. }) = &**callee_expr {
                if let Expr::Ident(id) = &**obj {
                    if id.sym == *sym {
                        return Some(Expr::Ident(id.clone()));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// From a binary `x == null` or `null == x`, extract the non-null operand.
fn extract_loose_null_operand(left: &Box<Expr>, right: &Box<Expr>) -> Option<Expr> {
    if matches!(&**right, Expr::Lit(Lit::Null(_))) || is_undefined(right) {
        return Some((**left).clone());
    }
    if matches!(&**left, Expr::Lit(Lit::Null(_))) || is_undefined(left) {
        return Some((**right).clone());
    }
    None
}

// ---------------------------------------------------------------------------
// Null-check extraction helpers
// ---------------------------------------------------------------------------

struct NullCheckResult {
    value: Box<Expr>,
    real_value: Option<Box<Expr>>,
}

/// Extract from `x === null || x === void 0`.
/// Handles assignment form `(tmp = expr) === null || tmp === void 0`.
fn extract_null_check(expr: &Expr) -> Option<NullCheckResult> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };

    let left_val = extract_null_single(left)?;
    let right_val = extract_undefined_single(right)?;

    if let Some((tmp_sym, real_rhs)) = extract_assign_parts(&left_val) {
        if let Expr::Ident(ri) = &*right_val {
            if ri.sym == tmp_sym {
                return Some(NullCheckResult {
                    value: Box::new(Expr::Ident(ri.clone())),
                    real_value: Some(real_rhs.clone()),
                });
            }
        }
        return None;
    }

    if !exprs_structurally_equal(&left_val, &right_val) {
        return None;
    }

    Some(NullCheckResult {
        value: left_val,
        real_value: None,
    })
}

/// Extract from `x !== null && x !== void 0`.
fn extract_null_single(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::EqEqEq | BinaryOp::EqEq) {
        return None;
    }
    if matches!(&**right, Expr::Lit(Lit::Null(_))) {
        return Some(left.clone());
    }
    if matches!(&**left, Expr::Lit(Lit::Null(_))) {
        return Some(right.clone());
    }
    None
}

fn extract_undefined_single(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::EqEqEq | BinaryOp::EqEq) {
        return None;
    }
    if is_undefined(right) {
        return Some(left.clone());
    }
    if is_undefined(left) {
        return Some(right.clone());
    }
    None
}

fn is_void_or_undefined(expr: &Expr) -> bool {
    is_undefined(expr)
}

fn make_optional_call_replacing(
    real_rhs: &Expr,
    args: &[swc_core::ecma::ast::ExprOrSpread],
    type_args: &Option<Box<swc_core::ecma::ast::TsTypeParamInstantiation>>,
    span: swc_core::common::Span,
    ctxt: swc_core::common::SyntaxContext,
) -> Option<Expr> {
    let (context, call_args) = args.split_first()?;
    let recovered_callee = recover_babel_optional_call_callee(real_rhs, context.expr.as_ref())?;
    Some(Expr::OptChain(OptChainExpr {
        span: DUMMY_SP,
        optional: true,
        base: Box::new(OptChainBase::Call(OptCall {
            span,
            ctxt,
            callee: Box::new(recovered_callee),
            args: call_args.to_vec(),
            type_args: type_args.clone(),
        })),
    }))
}

fn extract_optional_call_target(callee: &Expr, context: &Expr) -> Option<Expr> {
    match strip_parens(callee) {
        Expr::Member(MemberExpr { obj, prop, .. }) if prop.is_ident_with("call") => {
            optional_call_context_matches_base(obj, context).then(|| (**obj).clone())
        }
        Expr::OptChain(OptChainExpr { base, .. }) => match base.as_ref() {
            OptChainBase::Member(MemberExpr { obj, prop, .. }) if prop.is_ident_with("call") => {
                optional_call_context_matches_base(obj, context).then(|| (**obj).clone())
            }
            _ => None,
        },
        _ => None,
    }
}

fn optional_call_context_matches_base(base: &Expr, context: &Expr) -> bool {
    match strip_parens(base) {
        Expr::Member(MemberExpr { obj, .. }) => recover_babel_call_context(obj, context).is_some(),
        Expr::OptChain(OptChainExpr { base: opt_base, .. }) => match opt_base.as_ref() {
            OptChainBase::Member(MemberExpr { obj, .. }) => {
                recover_babel_call_context(obj, context).is_some()
            }
            _ => false,
        },
        _ => false,
    }
}

fn recover_babel_optional_call_callee(real_rhs: &Expr, context: &Expr) -> Option<Expr> {
    match strip_parens(real_rhs) {
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            let recovered_obj = recover_babel_call_context(obj, context)?;
            Some(Expr::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(recovered_obj),
                prop: prop.clone(),
            }))
        }
        Expr::OptChain(OptChainExpr { base, .. }) => match base.as_ref() {
            OptChainBase::Member(MemberExpr { obj, .. })
                if recover_babel_call_context(obj, context).is_some()
                    || is_babel_temp_expr(context) =>
            {
                Some(real_rhs.clone())
            }
            _ => None,
        },
        _ => {
            let recovered = recover_lowered_optional_chain_expr(real_rhs)?;
            if let Some(expected_context) = recovered.expected_context.as_ref() {
                if !exprs_structurally_equal(context, expected_context) {
                    return None;
                }
            }
            Some(recovered.chain)
        }
    }
}

struct RecoveredLoweredOptionalChain {
    chain: Expr,
    expected_context: Option<Expr>,
}

fn recover_lowered_optional_chain_expr(expr: &Expr) -> Option<RecoveredLoweredOptionalChain> {
    if let Some(recovered) = recover_strict_lowered_optional_chain_expr(expr) {
        return Some(recovered);
    }
    recover_loose_lowered_optional_chain_expr(expr)
}

fn recover_strict_lowered_optional_chain_expr(
    expr: &Expr,
) -> Option<RecoveredLoweredOptionalChain> {
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = strip_parens(expr)
    else {
        return None;
    };

    if !is_void_or_undefined(cons) {
        return None;
    }

    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(test)?;

    let expected_context = Some((*checked).clone());
    let chain = if let Some(real_rhs) = real_value {
        make_optional_chain_replacing(&checked, &real_rhs, alt)?
    } else {
        make_optional_chain(*checked, alt)?
    };

    Some(RecoveredLoweredOptionalChain {
        chain,
        expected_context,
    })
}

fn recover_loose_lowered_optional_chain_expr(expr: &Expr) -> Option<RecoveredLoweredOptionalChain> {
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = &**test
    else {
        return None;
    };

    match op {
        BinaryOp::EqEq => {
            if !is_void_or_undefined(cons) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right)?;
            recover_loose_lowered_optional_chain_parts(checked, alt)
        }
        BinaryOp::NotEq => {
            if !is_void_or_undefined(alt) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right)?;
            recover_loose_lowered_optional_chain_parts(checked, cons)
        }
        _ => None,
    }
}

fn recover_loose_lowered_optional_chain_parts(
    checked: Expr,
    access: &Expr,
) -> Option<RecoveredLoweredOptionalChain> {
    if let Some((tmp_sym, real_rhs)) = extract_assign_parts(&checked) {
        let tmp_ident_expr = find_ident_by_sym(access, &tmp_sym)?;
        Some(RecoveredLoweredOptionalChain {
            chain: make_optional_chain_replacing(&tmp_ident_expr, real_rhs, access)?,
            expected_context: Some(tmp_ident_expr),
        })
    } else {
        Some(RecoveredLoweredOptionalChain {
            chain: make_optional_chain(checked, access)?,
            expected_context: None,
        })
    }
}

fn recover_babel_call_context(member_obj: &Expr, context: &Expr) -> Option<Expr> {
    if exprs_structurally_equal(member_obj, context) {
        return Some(member_obj.clone());
    }

    let Expr::Ident(context_ident) = strip_parens(context) else {
        return None;
    };
    let (assigned_sym, assigned_rhs) = extract_assign_parts(member_obj)?;
    if assigned_sym == context_ident.sym {
        return Some((**assigned_rhs).clone());
    }

    None
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => strip_parens(&p.expr),
        _ => expr,
    }
}

fn extract_assign_parts(expr: &Expr) -> Option<(swc_core::atoms::Atom, &Box<Expr>)> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left else {
        return None;
    };
    Some((id.id.sym.clone(), &assign.right))
}

fn is_standard_temp_expr(
    expr: &Expr,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    expected_references: usize,
) -> bool {
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(expr) else {
        return false;
    };
    let binding_id = (sym.clone(), *ctxt);
    is_babel_temp_sym(sym)
        || (uninitialized_bindings.contains(&binding_id)
            && binding_references.get(&binding_id).copied() == Some(expected_references))
}

fn is_nested_optional_chain_temp_expr(
    checked: &Expr,
    real_rhs: &Expr,
    binding_references: &HashMap<BindingId, usize>,
    expected_references: usize,
) -> bool {
    if !matches!(strip_parens(real_rhs), Expr::OptChain(_)) {
        return false;
    }
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(checked) else {
        return false;
    };
    let binding_id = (sym.clone(), *ctxt);
    looks_generated_temp_sym(sym)
        && binding_references.get(&binding_id).copied() == Some(expected_references)
}

fn is_generated_member_temp_expr(
    checked: &Expr,
    real_rhs: &Expr,
    binding_references: &HashMap<BindingId, usize>,
    expected_references: usize,
) -> bool {
    if !matches!(strip_parens(real_rhs), Expr::Member(_)) {
        return false;
    }
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(checked) else {
        return false;
    };
    let binding_id = (sym.clone(), *ctxt);
    looks_generated_temp_sym(sym)
        && binding_references.get(&binding_id).copied() == Some(expected_references)
}

fn is_babel_temp_expr(expr: &Expr) -> bool {
    let Expr::Ident(Ident { sym, .. }) = strip_parens(expr) else {
        return false;
    };
    is_babel_temp_sym(sym)
}

fn is_babel_temp_sym(sym: &swc_core::atoms::Atom) -> bool {
    sym.starts_with('_')
}

fn looks_generated_temp_sym(sym: &swc_core::atoms::Atom) -> bool {
    is_babel_temp_sym(sym) || sym.chars().any(|ch| ch.is_ascii_digit())
}

#[derive(Default)]
struct TempBindingFactsCollector {
    uninitialized: HashSet<BindingId>,
    references: HashMap<BindingId, usize>,
}

impl Visit for TempBindingFactsCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_none() {
            let Pat::Ident(binding) = &declarator.name else {
                declarator.visit_children_with(self);
                return;
            };
            self.uninitialized
                .insert((binding.id.sym.clone(), binding.id.ctxt));
        }
        declarator.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let binding_id = (ident.sym.clone(), ident.ctxt);
        *self.references.entry(binding_id).or_insert(0) += 1;
    }
}

struct TempBindingFacts {
    uninitialized: HashSet<BindingId>,
    references: HashMap<BindingId, usize>,
}

fn collect_temp_binding_facts(module: &Module) -> TempBindingFacts {
    let mut collector = TempBindingFactsCollector::default();
    module.visit_with(&mut collector);
    TempBindingFacts {
        uninitialized: collector.uninitialized,
        references: collector.references,
    }
}
