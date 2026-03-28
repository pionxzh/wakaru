use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, CondExpr, Expr, Lit, MemberExpr,
    OptCall, OptChainBase, OptChainExpr, SimpleAssignTarget,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::un_nullish_coalescing::{exprs_structurally_equal, is_undefined};

pub struct UnOptionalChaining;

impl VisitMut for UnOptionalChaining {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(result) = try_optional_chaining(expr) {
            *expr = result;
        }
    }
}

fn try_optional_chaining(expr: &Expr) -> Option<Expr> {
    // Pattern: `(obj === null || obj === void 0) ? void 0 : obj.access`
    if let Some(result) = try_ternary_optional_chain(expr) {
        return Some(result);
    }

    None
}

/// Handle: `(obj === null || obj === void 0) ? void 0 : obj.access`  →  `obj?.access`
/// Also handles assignment form: `(tmp = expr) === null || tmp === void 0 ? void 0 : tmp.access`
fn try_ternary_optional_chain(expr: &Expr) -> Option<Expr> {
    let Expr::Cond(CondExpr { test, cons, alt, .. }) = expr else {
        return None;
    };

    // consequent must be `void 0` or `undefined`
    if !is_void_or_undefined(cons) {
        return None;
    }

    // test must be `x === null || x === void 0`
    let NullCheckResult { value: checked, real_value } = extract_null_check(test)?;

    if let Some(real_rhs) = real_value {
        // Assignment form: `checked` is `tmp`, `real_rhs` is the original expr
        // alt must use `tmp` as the object
        let chain = make_optional_chain_replacing(&checked, &real_rhs, alt)?;
        return Some(chain);
    }

    // Plain form
    make_optional_chain(*checked, alt)
}

/// Build `base?.prop` or `base?.method(...)` where `access` uses `base` as its object.
fn make_optional_chain(base: Expr, access: &Expr) -> Option<Expr> {
    match access {
        // x.prop → x?.prop
        Expr::Member(MemberExpr { obj, prop, .. })
            if exprs_structurally_equal(obj, &base) =>
        {
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
fn make_optional_chain_replacing(
    tmp: &Expr,
    real_rhs: &Expr,
    access: &Expr,
) -> Option<Expr> {
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
    let Expr::Bin(BinExpr { op, left, right, .. }) = expr else {
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
    let Expr::Bin(BinExpr { op, left, right, .. }) = expr else {
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
