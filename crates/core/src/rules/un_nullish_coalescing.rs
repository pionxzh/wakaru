use swc_core::common::{Mark, SyntaxContext};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CondExpr, Expr, Lit, SimpleAssignTarget,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub(crate) use super::expr_utils::{exprs_structurally_equal, is_unresolved_undefined};

pub struct UnNullishCoalescing {
    unresolved_mark: Mark,
}

impl UnNullishCoalescing {
    pub fn new(unresolved_mark: Mark) -> Self {
        Self { unresolved_mark }
    }
}

impl VisitMut for UnNullishCoalescing {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(result) = try_nullish_coalescing(expr, self.unresolved_mark) {
            *expr = result;
        }
    }
}

fn try_nullish_coalescing(expr: &Expr, unresolved_mark: Mark) -> Option<Expr> {
    let Expr::Cond(cond_expr) = expr else {
        return None;
    };

    // Pattern B: `x === null || x === void 0 ? fallback : x`
    if let Some(result) = try_pattern_b_coalescing(cond_expr, unresolved_mark) {
        return Some(result);
    }

    // Pattern A: `x !== null && x !== void 0 ? x : fallback`
    if let Some(result) = try_pattern_a_coalescing(cond_expr, unresolved_mark) {
        return Some(result);
    }

    None
}

/// Pattern A: `x !== null && x !== void 0 ? x : fallback`  →  `x ?? fallback`
/// Temp-var form: `(tmp = expr) !== null && tmp !== void 0 ? tmp : fallback`  →  `expr ?? fallback`
fn try_pattern_a_coalescing(cond: &CondExpr, unresolved_mark: Mark) -> Option<Expr> {
    let NullCheckResult { value, real_value } =
        extract_not_null_check(&cond.test, unresolved_mark)?;

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
fn try_pattern_b_coalescing(cond: &CondExpr, unresolved_mark: Mark) -> Option<Expr> {
    let NullCheckResult { value, real_value } = extract_null_check(&cond.test, unresolved_mark)?;

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
fn extract_not_null_check(expr: &Expr, unresolved_mark: Mark) -> Option<NullCheckResult> {
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
    let right_val = extract_not_undefined_single(right, unresolved_mark)?;

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
fn extract_null_check(expr: &Expr, unresolved_mark: Mark) -> Option<NullCheckResult> {
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
    let right_val = extract_undefined_single(right, unresolved_mark)?;

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
fn extract_not_undefined_single(expr: &Expr, unresolved_mark: Mark) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::NotEqEq | BinaryOp::NotEq) {
        return None;
    }
    if is_unresolved_undefined(right, unresolved_mark) {
        return Some(left.clone());
    }
    if is_unresolved_undefined(left, unresolved_mark) {
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
fn extract_undefined_single(expr: &Expr, unresolved_mark: Mark) -> Option<Box<Expr>> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = expr
    else {
        return None;
    };
    if !matches!(op, BinaryOp::EqEqEq | BinaryOp::EqEq) {
        return None;
    }
    if is_unresolved_undefined(right, unresolved_mark) {
        return Some(left.clone());
    }
    if is_unresolved_undefined(left, unresolved_mark) {
        return Some(right.clone());
    }
    None
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

fn make_nullish_coalescing(value: Box<Expr>, fallback: Box<Expr>) -> Expr {
    (*value).make_bin(BinaryOp::NullishCoalescing, *fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swc_core::common::{Mark, DUMMY_SP, GLOBALS};
    use swc_core::ecma::ast::{ComputedPropName, Ident, MemberProp};

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
