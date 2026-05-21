use std::collections::{HashMap, HashSet};

use swc_core::common::{Mark, SyntaxContext, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, Bool, CondExpr, Expr, Ident, Lit, Module, Pat,
    SimpleAssignTarget, VarDeclarator,
};
use swc_core::ecma::utils::ExprFactory;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

pub(crate) use super::expr_utils::{exprs_structurally_equal, is_unresolved_undefined};
use super::{RewriteLevel, RewritePolicy};

type BindingId = (swc_core::atoms::Atom, SyntaxContext);

pub struct UnNullishCoalescing {
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: HashSet<BindingId>,
    binding_references: HashMap<BindingId, usize>,
}

impl UnNullishCoalescing {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            policy: RewritePolicy::from_level(level),
            uninitialized_bindings: HashSet::new(),
            binding_references: HashMap::new(),
        }
    }
}

impl VisitMut for UnNullishCoalescing {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let facts = collect_binding_facts(module);
        self.uninitialized_bindings = facts.uninitialized;
        self.binding_references = facts.references;
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if let Some(result) = try_nullish_coalescing(
            expr,
            self.unresolved_mark,
            self.policy,
            &self.uninitialized_bindings,
            &self.binding_references,
        ) {
            *expr = result;
        }
    }
}

fn try_nullish_coalescing(
    expr: &Expr,
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    // Pattern C: `(tmp = X) === null || tmp === undefined || tmp` → `X ?? true`
    // (||‐chain encoding of nullish coalescing with boolean `true` default)
    if let Some(result) = try_pattern_c_coalescing(
        expr,
        unresolved_mark,
        policy,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    let Expr::Cond(cond_expr) = expr else {
        return None;
    };

    // Pattern D: `x != null ? x : fallback` / `x == null ? fallback : x`
    // Temp-var form: `(tmp = expr) != null ? tmp : fallback` -> `expr ?? fallback`
    if policy.assumptions.no_document_all {
        if let Some(result) = try_loose_pattern_coalescing(
            cond_expr,
            unresolved_mark,
            uninitialized_bindings,
            binding_references,
        ) {
            return Some(result);
        }
    }

    // Pattern B: `x === null || x === void 0 ? fallback : x`
    if let Some(result) = try_pattern_b_coalescing(
        cond_expr,
        unresolved_mark,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    // Pattern A: `x !== null && x !== void 0 ? x : fallback`
    if let Some(result) = try_pattern_a_coalescing(
        cond_expr,
        unresolved_mark,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    None
}

fn try_loose_pattern_coalescing(
    cond: &CondExpr,
    unresolved_mark: Mark,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = cond.test.as_ref()
    else {
        return None;
    };

    match op {
        BinaryOp::NotEq => {
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            try_loose_pattern_parts(
                checked,
                &cond.cons,
                cond.alt.clone(),
                uninitialized_bindings,
                binding_references,
            )
        }
        BinaryOp::EqEq => {
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            try_loose_pattern_parts(
                checked,
                &cond.alt,
                cond.cons.clone(),
                uninitialized_bindings,
                binding_references,
            )
        }
        _ => None,
    }
}

fn try_loose_pattern_parts(
    checked: Box<Expr>,
    value_branch: &Expr,
    fallback: Box<Expr>,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    if let Some((tmp_sym, tmp_ctxt, real_rhs)) = extract_assign_parts(&checked) {
        let tmp = Box::new(Expr::Ident(Ident::new(tmp_sym.clone(), DUMMY_SP, tmp_ctxt)));
        if !exprs_structurally_equal(value_branch, &tmp) {
            return None;
        }
        if !is_safe_temp_with_total_refs(
            &tmp_sym,
            tmp_ctxt,
            uninitialized_bindings,
            binding_references,
            3,
        ) {
            return None;
        }
        return Some(make_nullish_coalescing(real_rhs.clone(), fallback));
    }

    if exprs_structurally_equal(value_branch, &checked) {
        return Some(make_nullish_coalescing(checked, fallback));
    }

    None
}

/// Pattern A: `x !== null && x !== void 0 ? x : fallback`  →  `x ?? fallback`
/// Temp-var form: `(tmp = expr) !== null && tmp !== void 0 ? tmp : fallback`  →  `expr ?? fallback`
fn try_pattern_a_coalescing(
    cond: &CondExpr,
    unresolved_mark: Mark,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    let NullCheckResult { value, real_value } =
        extract_not_null_check(&cond.test, unresolved_mark)?;

    // Temp-var form: consequent must equal `tmp` (the variable on the LHS of the assignment)
    if let Some(real) = real_value {
        if exprs_structurally_equal(&cond.cons, &value) {
            if let Expr::Ident(ref tmp_ident) = *value {
                if !is_safe_temp(
                    &tmp_ident.sym,
                    tmp_ident.ctxt,
                    uninitialized_bindings,
                    binding_references,
                ) {
                    return None;
                }
            }
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
fn try_pattern_b_coalescing(
    cond: &CondExpr,
    unresolved_mark: Mark,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    let NullCheckResult { value, real_value } = extract_null_check(&cond.test, unresolved_mark)?;

    // Temp-var form: alternate must equal `tmp`
    if let Some(real) = real_value {
        if exprs_structurally_equal(&cond.alt, &value) {
            if let Expr::Ident(ref tmp_ident) = *value {
                if !is_safe_temp(
                    &tmp_ident.sym,
                    tmp_ident.ctxt,
                    uninitialized_bindings,
                    binding_references,
                ) {
                    return None;
                }
            }
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

/// Pattern C: `(tmp = X) === null || tmp === undefined || tmp`  →  `X ?? true`
/// Also handles the plain identifier form: `x === null || x === undefined || x`  →  `x ?? true`
///
/// This is an ||‐chain encoding of nullish coalescing where the default is `true`.
/// When X is null/undefined the `=== null` or `=== undefined` comparison yields `true`
/// (short-circuiting the chain), otherwise the chain falls through to `|| X` itself.
///
/// The plain form with member expressions (`B.broadcast === null || ...`) reads
/// the property up to 3 times, so collapsing to `B.broadcast ?? true` changes
/// semantics for getters, proxies, or values that change between reads. This
/// form requires `Aggressive` level. Plain identifiers may also differ under
/// `with` or global accessor bindings (3 reads → 1), but this is acceptable
/// under Wakaru's heuristic policy for Standard level.
/// The real machine-generated pattern always uses a temp variable.
fn try_pattern_c_coalescing(
    expr: &Expr,
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    // Shape: `(A || B) || C` where A = null-check, B = undefined-check, C = value
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left: outer_left,
        right: tail,
        ..
    }) = expr
    else {
        return None;
    };

    // outer_left must be another `||`
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalOr,
        left: null_check_expr,
        right: undef_check_expr,
        ..
    }) = outer_left.as_ref()
    else {
        return None;
    };

    // Extract the null-check value (possibly with assignment)
    let null_val = extract_null_single(null_check_expr)?;
    // Extract the undefined-check value
    let undef_val = extract_undefined_single(undef_check_expr, unresolved_mark)?;

    // Check for assignment pattern: null_val may be `(tmp = real_expr)`
    if let Some((tmp_sym, tmp_ctxt, real_rhs)) = extract_assign_parts(&null_val) {
        // undef_check must reference `tmp`
        if let Expr::Ident(ri) = &*undef_val {
            if ri.sym == tmp_sym && ri.ctxt == tmp_ctxt {
                // tail must also be `tmp`
                if let Expr::Ident(ti) = tail.as_ref() {
                    if ti.sym == tmp_sym && ti.ctxt == tmp_ctxt {
                        if !is_safe_temp(
                            &tmp_sym,
                            tmp_ctxt,
                            uninitialized_bindings,
                            binding_references,
                        ) {
                            return None;
                        }
                        return Some(make_nullish_coalescing(
                            real_rhs.clone(),
                            Box::new(Expr::Lit(Lit::Bool(Bool {
                                span: DUMMY_SP,
                                value: true,
                            }))),
                        ));
                    }
                }
            }
        }
        return None;
    }

    // Plain identifier form: identifiers may technically differ under `with` or
    // global accessor bindings (3 reads → 1), but this is acceptable heuristically.
    if matches!(&*null_val, Expr::Ident(_)) && policy.level < RewriteLevel::Standard {
        return None;
    }

    // Member expressions and other complex forms require Aggressive level
    // because collapsing 3 reads into 1 changes semantics for getters/proxies.
    if !matches!(&*null_val, Expr::Ident(_)) && !policy.assumptions.pure_getters {
        return None;
    }

    if !exprs_structurally_equal(&null_val, &undef_val) {
        return None;
    }
    if !exprs_structurally_equal(&null_val, tail) {
        return None;
    }

    Some(make_nullish_coalescing(
        null_val,
        Box::new(Expr::Lit(Lit::Bool(Bool {
            span: DUMMY_SP,
            value: true,
        }))),
    ))
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

fn extract_loose_null_operand(
    left: &Expr,
    right: &Expr,
    unresolved_mark: Mark,
) -> Option<Box<Expr>> {
    if matches!(right, Expr::Lit(Lit::Null(_))) || is_unresolved_undefined(right, unresolved_mark) {
        return Some(Box::new(left.clone()));
    }
    if matches!(left, Expr::Lit(Lit::Null(_))) || is_unresolved_undefined(left, unresolved_mark) {
        return Some(Box::new(right.clone()));
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

/// Check that a temp variable has a known uninitialized declaration (`var tmp;`
/// or `let tmp;`) and is only referenced within the pattern itself plus that
/// declaration. The temp appears 3 times in the pattern (assignment target +
/// 2 uses) and 1 time in the declaration, for a total of 4.
///
/// Without a known declaration the temp is either an undeclared global (sloppy
/// mode: erasing the assignment drops a global side effect) or an unresolved
/// binding (strict/module mode: the original throws `ReferenceError` but the
/// rewrite would not). Both cases are unsafe to erase.
fn is_safe_temp(
    sym: &swc_core::atoms::Atom,
    ctxt: SyntaxContext,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> bool {
    is_safe_temp_with_total_refs(sym, ctxt, uninitialized_bindings, binding_references, 4)
}

fn is_safe_temp_with_total_refs(
    sym: &swc_core::atoms::Atom,
    ctxt: SyntaxContext,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    expected_total_refs: usize,
) -> bool {
    let binding_id = (sym.clone(), ctxt);
    if !uninitialized_bindings.contains(&binding_id) {
        return false;
    }
    let total_refs = binding_references.get(&binding_id).copied().unwrap_or(0);
    total_refs == expected_total_refs
}

#[derive(Default)]
struct BindingFactsCollector {
    uninitialized: HashSet<BindingId>,
    references: HashMap<BindingId, usize>,
}

impl Visit for BindingFactsCollector {
    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if declarator.init.is_none() {
            if let Pat::Ident(binding) = &declarator.name {
                self.uninitialized
                    .insert((binding.id.sym.clone(), binding.id.ctxt));
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        let binding_id = (ident.sym.clone(), ident.ctxt);
        *self.references.entry(binding_id).or_insert(0) += 1;
    }
}

struct BindingFacts {
    uninitialized: HashSet<BindingId>,
    references: HashMap<BindingId, usize>,
}

fn collect_binding_facts(module: &Module) -> BindingFacts {
    let mut collector = BindingFactsCollector::default();
    module.visit_with(&mut collector);
    BindingFacts {
        uninitialized: collector.uninitialized,
        references: collector.references,
    }
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
