use swc_core::common::{SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CondExpr, Expr, Lit, MemberProp, SimpleAssignTarget,
    UnaryOp,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub struct UnNullishCoalescing;

impl VisitMut for UnNullishCoalescing {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(result) = try_nullish_coalescing(expr) {
            *expr = result;
        }
    }
}

fn try_nullish_coalescing(expr: &Expr) -> Option<Expr> {
    let Expr::Cond(cond_expr) = expr else {
        return None;
    };

    // Pattern B: `x === null || x === void 0 ? fallback : x`
    if let Some(result) = try_pattern_b_coalescing(cond_expr) {
        return Some(result);
    }

    // Pattern A: `x !== null && x !== void 0 ? x : fallback`
    if let Some(result) = try_pattern_a_coalescing(cond_expr) {
        return Some(result);
    }

    None
}

/// Pattern A: `x !== null && x !== void 0 ? x : fallback`  →  `x ?? fallback`
/// Temp-var form: `(tmp = expr) !== null && tmp !== void 0 ? tmp : fallback`  →  `expr ?? fallback`
fn try_pattern_a_coalescing(cond: &CondExpr) -> Option<Expr> {
    let NullCheckResult { value, real_value } = extract_not_null_check(&cond.test)?;

    // Temp-var form: consequent must equal `tmp` (the variable on the LHS of the assignment)
    if let Some(real) = real_value {
        if exprs_structurally_equal(&cond.cons, &value) {
            return Some(make_nullish_coalescing(real, cond.alt.clone()));
        }
        return None;
    }

    // Plain form: consequent must equal the checked value
    if exprs_structurally_equal(&cond.cons, &value) {
        return Some(make_nullish_coalescing(value, cond.alt.clone()));
    }

    None
}

/// Pattern B: `x === null || x === void 0 ? fallback : x`  →  `x ?? fallback`
/// Temp-var form: `(tmp = expr) === null || tmp === void 0 ? fallback : tmp`  →  `expr ?? fallback`
fn try_pattern_b_coalescing(cond: &CondExpr) -> Option<Expr> {
    let NullCheckResult { value, real_value } = extract_null_check(&cond.test)?;

    // Temp-var form: alternate must equal `tmp`
    if let Some(real) = real_value {
        if exprs_structurally_equal(&cond.alt, &value) {
            return Some(make_nullish_coalescing(real, cond.cons.clone()));
        }
        return None;
    }

    // Plain form: alternate must equal the checked value
    if exprs_structurally_equal(&cond.alt, &value) {
        return Some(make_nullish_coalescing(value, cond.cons.clone()));
    }

    None
}

/// Result of a null-check extraction.
/// `value` is the expression that was checked (or the tmp var ident if assignment).
/// `real_value` is `Some(rhs)` if the first operand was an assignment `(tmp = rhs)`.
struct NullCheckResult {
    /// The checked expression (ident/member/etc.). For assignment patterns, this is `tmp`.
    value: Box<Expr>,
    /// `Some(rhs)` when the left side of the null check was `(tmp = rhs)`.
    real_value: Option<Box<Expr>>,
}

/// Extract from `x !== null && x !== void 0`.
/// Also handles the assignment form `(tmp = expr) !== null && tmp !== void 0`.
fn extract_not_null_check(expr: &Expr) -> Option<NullCheckResult> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalAnd,
        left,
        right,
        ..
    }) = expr
    else {
        return None;
    };

    let left_val = extract_not_null_single(left)?;
    let right_val = extract_not_undefined_single(right)?;

    // Check for assignment pattern: left_val may be `(tmp = real_expr)`
    if let Some((tmp_sym, tmp_ctxt, real_rhs)) = extract_assign_parts(&left_val) {
        // right side must reference `tmp`
        if let Expr::Ident(ri) = &*right_val {
            if ri.sym == tmp_sym && ri.ctxt == tmp_ctxt {
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

/// Extract from `x === null || x === void 0`.
/// Also handles the assignment form `(tmp = expr) === null || tmp === void 0`.
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

    // Check for assignment pattern: left_val may be `(tmp = real_expr)`
    if let Some((tmp_sym, tmp_ctxt, real_rhs)) = extract_assign_parts(&left_val) {
        // right side must reference `tmp`
        if let Expr::Ident(ri) = &*right_val {
            if ri.sym == tmp_sym && ri.ctxt == tmp_ctxt {
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

/// Match `x !== null` or `null !== x` — return x.
fn extract_not_null_single(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::NotEqEq | BinaryOp::NotEq) {
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

/// Match `x !== void 0` or `x !== undefined` or flipped — return x.
fn extract_not_undefined_single(expr: &Expr) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::NotEqEq | BinaryOp::NotEq) {
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

/// Match `x === null` or `null === x` — return x.
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

/// Match `x === void 0` or `x === undefined` or flipped — return x.
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

pub(crate) fn is_undefined(expr: &Expr) -> bool {
    if matches!(expr, Expr::Ident(i) if &*i.sym == "undefined") {
        return true;
    }
    if let Expr::Unary(u) = expr {
        if u.op == UnaryOp::Void {
            if let Expr::Lit(Lit::Num(n)) = &*u.arg {
                return n.value == 0.0;
            }
        }
    }
    false
}

/// Strip parentheses from an expression.
fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => strip_parens(&p.expr),
        _ => expr,
    }
}

/// If `expr` is `(tmp = real_expr)` (parens allowed), return `(tmp_sym, tmp_ctxt, &real_expr)`.
fn extract_assign_parts(expr: &Expr) -> Option<(swc_core::atoms::Atom, SyntaxContext, &Box<Expr>)> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left else {
        return None;
    };
    Some((id.id.sym.clone(), id.id.ctxt, &assign.right))
}

pub(crate) fn exprs_structurally_equal(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Ident(ai), Expr::Ident(bi)) => ai.sym == bi.sym && ai.ctxt == bi.ctxt,
        (Expr::This(_), Expr::This(_)) => true,
        (Expr::Member(am), Expr::Member(bm)) => {
            exprs_structurally_equal(&am.obj, &bm.obj) && member_props_equal(&am.prop, &bm.prop)
        }
        _ => false,
    }
}

fn member_props_equal(a: &MemberProp, b: &MemberProp) -> bool {
    match (a, b) {
        (MemberProp::Ident(ai), MemberProp::Ident(bi)) => ai.sym == bi.sym,
        (MemberProp::Computed(ac), MemberProp::Computed(bc)) => {
            exprs_structurally_equal(&ac.expr, &bc.expr)
        }
        _ => false,
    }
}

fn make_nullish_coalescing(value: Box<Expr>, fallback: Box<Expr>) -> Expr {
    Expr::Bin(BinExpr {
        span: DUMMY_SP,
        op: BinaryOp::NullishCoalescing,
        left: value,
        right: fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{Mark, GLOBALS};
    use swc_core::ecma::ast::{ComputedPropName, Ident};

    fn ident(sym: &str, ctxt: SyntaxContext) -> Expr {
        Expr::Ident(Ident::new(sym.into(), DUMMY_SP, ctxt))
    }

    #[test]
    fn structural_expr_equality_distinguishes_identifier_contexts() {
        GLOBALS.set(&Default::default(), || {
            let first_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let second_ctxt = SyntaxContext::empty().apply_mark(Mark::new());

            assert!(exprs_structurally_equal(
                &ident("tmp", first_ctxt),
                &ident("tmp", first_ctxt)
            ));
            assert!(!exprs_structurally_equal(
                &ident("tmp", first_ctxt),
                &ident("tmp", second_ctxt)
            ));
        });
    }

    #[test]
    fn structural_member_equality_distinguishes_computed_identifier_contexts() {
        GLOBALS.set(&Default::default(), || {
            let first_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let second_ctxt = SyntaxContext::empty().apply_mark(Mark::new());

            let first = Expr::Member(swc_core::ecma::ast::MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(ident("obj", SyntaxContext::empty())),
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(ident("key", first_ctxt)),
                }),
            });
            let second = Expr::Member(swc_core::ecma::ast::MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(ident("obj", SyntaxContext::empty())),
                prop: MemberProp::Computed(ComputedPropName {
                    span: DUMMY_SP,
                    expr: Box::new(ident("key", second_ctxt)),
                }),
            });

            assert!(!exprs_structurally_equal(&first, &second));
        });
    }
}
