use std::collections::{HashMap, HashSet};

use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, CondExpr, Expr, Ident, IfStmt,
    Lit, MemberExpr, MemberProp, Module, OptCall, OptChainBase, OptChainExpr, SimpleAssignTarget,
    Stmt, UnaryExpr, UnaryOp,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::binding_facts::{binding_id, collect_binding_facts, BindingId};
use super::expr_utils::{exprs_structurally_equal, is_unresolved_undefined};
use super::{RewriteLevel, RewritePolicy};

pub struct UnOptionalChaining {
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: HashSet<BindingId>,
    binding_references: HashMap<BindingId, usize>,
}

impl UnOptionalChaining {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            policy: RewritePolicy::from_level(level),
            uninitialized_bindings: HashSet::new(),
            binding_references: HashMap::new(),
        }
    }
}

impl VisitMut for UnOptionalChaining {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let facts = collect_binding_facts(module);
        self.uninitialized_bindings = facts.uninitialized;
        self.binding_references = facts.references;
        module.visit_mut_children_with(self);
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Some(result) = try_optional_chaining(
            expr,
            self.unresolved_mark,
            self.policy,
            &self.uninitialized_bindings,
            &self.binding_references,
        ) {
            *expr = result;
            expr.visit_mut_children_with(self);
            if let Some(result) = try_optional_call_cleanup(expr) {
                *expr = result;
            }
            return;
        }

        expr.visit_mut_children_with(self);

        if let Some(result) = try_optional_call_cleanup(expr) {
            *expr = result;
            return;
        }

        if let Some(result) = try_optional_chaining(
            expr,
            self.unresolved_mark,
            self.policy,
            &self.uninitialized_bindings,
            &self.binding_references,
        ) {
            *expr = result;
        }
    }

    fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
        stmt.visit_mut_children_with(self);

        if let Some(result) = try_optional_call_short_circuit_stmt(stmt, self.unresolved_mark) {
            *stmt = result;
            return;
        }

        if let Some(result) = try_optional_call_if_stmt(stmt, self.unresolved_mark) {
            *stmt = result;
        }
    }
}

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

fn try_optional_call_if_stmt(stmt: &Stmt, unresolved_mark: Mark) -> Option<Stmt> {
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
    } = extract_null_check(strip_parens(arg), unresolved_mark)?;
    let real_rhs = real_value?;
    let call_expr = extract_single_call_expr(cons)?;
    build_optional_call_stmt(DUMMY_SP, &checked, &real_rhs, call_expr, unresolved_mark)
}

fn try_optional_call_short_circuit_stmt(stmt: &Stmt, unresolved_mark: Mark) -> Option<Stmt> {
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
    } = extract_null_check(left, unresolved_mark)?;
    let real_rhs = real_value?;
    build_optional_call_stmt(
        expr_stmt.span,
        &checked,
        &real_rhs,
        right.as_ref(),
        unresolved_mark,
    )
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
    unresolved_mark: Mark,
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
            if matches!(base.as_ref(), OptChainBase::Member(_))
                && optional_call_context_matches_base(real_rhs, context.expr.as_ref()) =>
        {
            real_rhs.clone()
        }
        _ => recover_babel_optional_call_callee(real_rhs, context.expr.as_ref(), unresolved_mark)?,
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
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    // Pattern: `(obj === null || obj === void 0) ? void 0 : obj.access`
    if let Some(result) = try_ternary_optional_chain(
        expr,
        unresolved_mark,
        policy,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    // Pattern: Babel-style flattened chains:
    // `(_a = obj) === null || _a === void 0 || (_a = _a.prop) === null || ... ? void 0 : _a.leaf`
    if let Some(result) = try_flattened_optional_chain(
        expr,
        unresolved_mark,
        policy,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    // Pattern: `obj == null ? undefined : obj.access`  (loose equality)
    if let Some(result) = try_loose_eq_optional_chain(
        expr,
        unresolved_mark,
        policy,
        uninitialized_bindings,
        binding_references,
    ) {
        return Some(result);
    }

    None
}

fn try_flattened_optional_chain(
    expr: &Expr,
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    if policy.level < RewriteLevel::Standard {
        return None;
    }

    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = expr
    else {
        return None;
    };
    if !is_void_or_undefined(cons, unresolved_mark) {
        return None;
    }

    try_flattened_strict_optional_chain(
        test,
        alt,
        policy,
        uninitialized_bindings,
        binding_references,
        unresolved_mark,
    )
    .or_else(|| {
        try_flattened_mixed_loose_root_optional_chain(
            test,
            alt,
            policy,
            uninitialized_bindings,
            binding_references,
            unresolved_mark,
        )
    })
    .or_else(|| {
        try_flattened_loose_optional_chain(
            test,
            alt,
            policy,
            uninitialized_bindings,
            binding_references,
            unresolved_mark,
        )
    })
}

fn try_flattened_strict_optional_chain(
    test: &Expr,
    alt: &Expr,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    let mut terms = Vec::new();
    collect_logical_or_terms(test, &mut terms);
    if terms.len() < 4 || terms.len() % 2 != 0 {
        return None;
    }

    let first = extract_null_single(terms[0])?;
    let (first_tmp, mut chain) =
        extract_flattened_assignment_segment(&first, terms[1], unresolved_mark)?;
    let mut temps = vec![first_tmp.clone()];
    let mut temp_values = HashMap::new();
    let mut temp_call_contexts = HashMap::new();
    record_flattened_temp_value(
        &first_tmp,
        &chain,
        None,
        &mut temp_values,
        &mut temp_call_contexts,
    );
    let mut current_tmp = Expr::Ident(first_tmp);

    let mut index = 2;
    while index < terms.len() {
        let segment = extract_null_single(terms[index])?;
        let (next_tmp, real_rhs) =
            extract_flattened_assignment_segment(&segment, terms[index + 1], unresolved_mark)?;
        chain = make_optional_chain_replacing(&current_tmp, &chain, &real_rhs, unresolved_mark)?;
        let call_context = flattened_member_call_context(&real_rhs, &temp_values);
        record_flattened_temp_value(
            &next_tmp,
            &chain,
            call_context,
            &mut temp_values,
            &mut temp_call_contexts,
        );
        current_tmp = Expr::Ident(next_tmp.clone());
        temps.push(next_tmp);
        index += 2;
    }

    if !flattened_chain_temps_are_safe(
        &temps,
        test,
        alt,
        policy.level,
        uninitialized_bindings,
        binding_references,
    ) {
        return None;
    }

    make_flattened_final_access(
        &current_tmp,
        &chain,
        alt,
        &temp_values,
        &temp_call_contexts,
        unresolved_mark,
    )
}

fn try_flattened_mixed_loose_root_optional_chain(
    test: &Expr,
    alt: &Expr,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if !policy.assumptions.no_document_all {
        return None;
    }

    let mut terms = Vec::new();
    collect_logical_or_terms(test, &mut terms);
    if terms.len() < 3 || terms.len() % 2 == 0 {
        return None;
    }

    let base = extract_loose_null_single(terms[0], unresolved_mark)?;
    if !matches!(strip_parens(&base), Expr::Ident(_)) && !policy.assumptions.pure_getters {
        return None;
    }

    let mut chain = base.clone();
    let mut current_tmp = base;
    let mut temps = Vec::new();
    let mut temp_values = HashMap::new();
    let mut temp_call_contexts = HashMap::new();

    let mut index = 1;
    while index < terms.len() {
        let segment = extract_null_single(terms[index])?;
        let (next_tmp, real_rhs) =
            extract_flattened_assignment_segment(&segment, terms[index + 1], unresolved_mark)?;
        chain = make_optional_chain_replacing(&current_tmp, &chain, &real_rhs, unresolved_mark)?;
        let call_context = flattened_member_call_context(&real_rhs, &temp_values);
        record_flattened_temp_value(
            &next_tmp,
            &chain,
            call_context,
            &mut temp_values,
            &mut temp_call_contexts,
        );
        current_tmp = Expr::Ident(next_tmp.clone());
        temps.push(next_tmp);
        index += 2;
    }

    if !flattened_chain_temps_are_safe(
        &temps,
        test,
        alt,
        policy.level,
        uninitialized_bindings,
        binding_references,
    ) {
        return None;
    }

    make_flattened_final_access(
        &current_tmp,
        &chain,
        alt,
        &temp_values,
        &temp_call_contexts,
        unresolved_mark,
    )
}

fn try_flattened_loose_optional_chain(
    test: &Expr,
    alt: &Expr,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if !policy.assumptions.no_document_all {
        return None;
    }

    let mut terms = Vec::new();
    collect_logical_or_terms(test, &mut terms);
    if terms.len() < 2 {
        return None;
    }

    let (first_tmp, mut chain) =
        extract_flattened_loose_assignment_segment(terms[0], unresolved_mark)?;
    let mut temps = vec![first_tmp.clone()];
    let mut temp_values = HashMap::new();
    let mut temp_call_contexts = HashMap::new();
    record_flattened_temp_value(
        &first_tmp,
        &chain,
        None,
        &mut temp_values,
        &mut temp_call_contexts,
    );
    let mut current_tmp = Expr::Ident(first_tmp);

    for (index, term) in terms.iter().enumerate().skip(1) {
        let Some((next_tmp, real_rhs)) =
            extract_flattened_loose_assignment_segment(term, unresolved_mark)
        else {
            if index == terms.len() - 1 && policy.assumptions.pure_getters {
                if !flattened_chain_temps_are_safe(
                    &temps,
                    test,
                    alt,
                    policy.level,
                    uninitialized_bindings,
                    binding_references,
                ) {
                    return None;
                }
                return make_flattened_loose_repeated_access(
                    &current_tmp,
                    &chain,
                    term,
                    alt,
                    unresolved_mark,
                );
            }
            return None;
        };
        chain = make_optional_chain_replacing(&current_tmp, &chain, &real_rhs, unresolved_mark)?;
        let call_context = flattened_member_call_context(&real_rhs, &temp_values);
        record_flattened_temp_value(
            &next_tmp,
            &chain,
            call_context,
            &mut temp_values,
            &mut temp_call_contexts,
        );
        current_tmp = Expr::Ident(next_tmp.clone());
        temps.push(next_tmp);
    }

    if !flattened_chain_temps_are_safe(
        &temps,
        test,
        alt,
        policy.level,
        uninitialized_bindings,
        binding_references,
    ) {
        return None;
    }

    make_flattened_final_access(
        &current_tmp,
        &chain,
        alt,
        &temp_values,
        &temp_call_contexts,
        unresolved_mark,
    )
}

fn make_flattened_loose_repeated_access(
    current_tmp: &Expr,
    chain: &Expr,
    term: &Expr,
    alt: &Expr,
    unresolved_mark: Mark,
) -> Option<Expr> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::EqEq,
        left,
        right,
        ..
    }) = strip_parens(term)
    else {
        return None;
    };
    let checked_access = extract_loose_null_operand(left, right, unresolved_mark)?;
    let optional_access =
        make_optional_chain_replacing(current_tmp, chain, &checked_access, unresolved_mark)?;

    if exprs_structurally_equal(alt, &checked_access) {
        return Some(optional_access);
    }

    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee_expr),
        args,
        type_args,
        span,
        ctxt,
    }) = strip_parens(alt)
    else {
        return None;
    };
    if !exprs_structurally_equal(callee_expr.as_ref(), &checked_access) {
        return None;
    }

    Some(Expr::OptChain(OptChainExpr {
        span: DUMMY_SP,
        optional: true,
        base: Box::new(OptChainBase::Call(OptCall {
            span: *span,
            ctxt: *ctxt,
            callee: Box::new(optional_access),
            args: args.clone(),
            type_args: type_args.clone(),
        })),
    }))
}

fn collect_logical_or_terms<'a>(expr: &'a Expr, terms: &mut Vec<&'a Expr>) {
    match strip_parens(expr) {
        Expr::Bin(BinExpr {
            op: BinaryOp::LogicalOr,
            left,
            right,
            ..
        }) => {
            collect_logical_or_terms(left, terms);
            collect_logical_or_terms(right, terms);
        }
        expr => terms.push(expr),
    }
}

fn extract_flattened_assignment_segment(
    null_value: &Expr,
    undefined_check: &Expr,
    unresolved_mark: Mark,
) -> Option<(Ident, Expr)> {
    let (tmp, real_rhs) = extract_assign_ident_parts(null_value)?;
    let undefined_value = extract_undefined_single(undefined_check, unresolved_mark)?;
    if !exprs_structurally_equal(&Expr::Ident(tmp.clone()), &undefined_value) {
        return None;
    }
    Some((tmp, *real_rhs.clone()))
}

fn extract_flattened_loose_assignment_segment(
    term: &Expr,
    unresolved_mark: Mark,
) -> Option<(Ident, Expr)> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::EqEq,
        left,
        right,
        ..
    }) = strip_parens(term)
    else {
        return None;
    };
    let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
    let (tmp, real_rhs) = extract_assign_ident_parts(&checked)?;
    Some((tmp, *real_rhs.clone()))
}

fn extract_loose_null_single(expr: &Expr, unresolved_mark: Mark) -> Option<Expr> {
    let Expr::Bin(BinExpr {
        op: BinaryOp::EqEq,
        left,
        right,
        ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    extract_loose_null_operand(left, right, unresolved_mark)
}

fn record_flattened_temp_value(
    tmp: &Ident,
    value: &Expr,
    call_context: Option<Expr>,
    temp_values: &mut HashMap<BindingId, Expr>,
    temp_call_contexts: &mut HashMap<BindingId, Expr>,
) {
    let binding_id = binding_id(tmp);
    temp_values.insert(binding_id.clone(), value.clone());
    if let Some(call_context) = call_context {
        temp_call_contexts.insert(binding_id, call_context);
    }
}

fn flattened_member_call_context(
    real_rhs: &Expr,
    temp_values: &HashMap<BindingId, Expr>,
) -> Option<Expr> {
    let Expr::Member(MemberExpr { obj, .. }) = strip_parens(real_rhs) else {
        return None;
    };
    Some(resolve_flattened_temp_expr(obj, temp_values))
}

fn make_flattened_final_access(
    current_tmp: &Expr,
    chain: &Expr,
    access: &Expr,
    temp_values: &HashMap<BindingId, Expr>,
    temp_call_contexts: &HashMap<BindingId, Expr>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if let Some(chain) = make_optional_chain_replacing(current_tmp, chain, access, unresolved_mark)
    {
        return Some(chain);
    }
    make_flattened_optional_call(current_tmp, chain, access, temp_values, temp_call_contexts)
}

fn make_flattened_optional_call(
    current_tmp: &Expr,
    chain: &Expr,
    access: &Expr,
    temp_values: &HashMap<BindingId, Expr>,
    temp_call_contexts: &HashMap<BindingId, Expr>,
) -> Option<Expr> {
    let Expr::Ident(current_ident) = strip_parens(current_tmp) else {
        return None;
    };
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee_expr),
        args,
        type_args,
        span,
        ctxt,
    }) = access
    else {
        return None;
    };
    let Expr::Member(MemberExpr { obj, prop, .. }) = callee_expr.as_ref() else {
        return None;
    };
    if !prop.is_ident_with("call") || !exprs_structurally_equal(obj, current_tmp) {
        return None;
    }

    let (context, call_args) = args.split_first()?;
    let binding_id = binding_id(current_ident);
    let expected_context = temp_call_contexts.get(&binding_id)?;
    let actual_context = resolve_flattened_temp_expr(context.expr.as_ref(), temp_values);
    if !flattened_call_contexts_equal(expected_context, &actual_context) {
        return None;
    }

    Some(Expr::OptChain(OptChainExpr {
        span: DUMMY_SP,
        optional: true,
        base: Box::new(OptChainBase::Call(OptCall {
            span: *span,
            ctxt: *ctxt,
            callee: Box::new(chain.clone()),
            args: call_args.to_vec(),
            type_args: type_args.clone(),
        })),
    }))
}

fn resolve_flattened_temp_expr(expr: &Expr, temp_values: &HashMap<BindingId, Expr>) -> Expr {
    let Expr::Ident(ident) = strip_parens(expr) else {
        return expr.clone();
    };
    temp_values
        .get(&binding_id(ident))
        .cloned()
        .unwrap_or_else(|| expr.clone())
}

fn flattened_call_contexts_equal(a: &Expr, b: &Expr) -> bool {
    if exprs_structurally_equal(a, b) {
        return true;
    }

    match (strip_parens(a), strip_parens(b)) {
        (
            Expr::OptChain(OptChainExpr {
                base: a_base,
                optional: a_optional,
                ..
            }),
            Expr::OptChain(OptChainExpr {
                base: b_base,
                optional: b_optional,
                ..
            }),
        ) if a_optional == b_optional => match (a_base.as_ref(), b_base.as_ref()) {
            (
                OptChainBase::Member(MemberExpr {
                    obj: a_obj,
                    prop: a_prop,
                    ..
                }),
                OptChainBase::Member(MemberExpr {
                    obj: b_obj,
                    prop: b_prop,
                    ..
                }),
            ) => {
                member_props_equal(a_prop, b_prop)
                    && flattened_call_contexts_equal(a_obj.as_ref(), b_obj.as_ref())
            }
            _ => false,
        },
        _ => false,
    }
}

fn member_props_equal(a: &MemberProp, b: &MemberProp) -> bool {
    match (a, b) {
        (MemberProp::Ident(ai), MemberProp::Ident(bi)) => ai.sym == bi.sym,
        (MemberProp::Computed(ac), MemberProp::Computed(bc)) => {
            flattened_call_contexts_equal(ac.expr.as_ref(), bc.expr.as_ref())
        }
        _ => false,
    }
}

fn flattened_chain_temps_are_safe(
    temps: &[Ident],
    test: &Expr,
    alt: &Expr,
    level: RewriteLevel,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> bool {
    let pattern_references = count_binding_references_in_exprs(&[test, alt]);
    let unique_temps: HashSet<BindingId> = temps.iter().map(binding_id).collect();

    unique_temps.iter().all(|binding_id| {
        let pattern_count = pattern_references.get(binding_id).copied().unwrap_or(0);
        let total_count = binding_references.get(binding_id).copied().unwrap_or(0);
        if uninitialized_bindings.contains(binding_id) && total_count == pattern_count + 1 {
            return true;
        }
        if level >= RewriteLevel::Aggressive
            && looks_generated_temp_sym(&binding_id.0)
            && total_count == pattern_count
        {
            return true;
        }
        false
    })
}

fn temp_expr_is_safe_for_pattern(
    temp: &Expr,
    exprs: &[&Expr],
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> bool {
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(temp) else {
        return false;
    };
    let binding_id = (sym.clone(), *ctxt);
    let pattern_references = count_binding_references_in_exprs(exprs)
        .get(&binding_id)
        .copied()
        .unwrap_or(0);
    let total_references = binding_references.get(&binding_id).copied().unwrap_or(0);

    if uninitialized_bindings.contains(&binding_id) && total_references == pattern_references + 1 {
        return true;
    }
    looks_generated_temp_sym(sym) && total_references == pattern_references
}

fn count_binding_references_in_exprs(exprs: &[&Expr]) -> HashMap<BindingId, usize> {
    let mut counter = BindingReferenceCounter::default();
    for expr in exprs {
        expr.visit_with(&mut counter);
    }
    counter.references
}

/// Handle: `(obj === null || obj === void 0) ? void 0 : obj.access`  →  `obj?.access`
/// Also handles assignment form: `(tmp = expr) === null || tmp === void 0 ? void 0 : tmp.access`
fn try_ternary_optional_chain(
    expr: &Expr,
    unresolved_mark: Mark,
    policy: RewritePolicy,
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
    if !is_void_or_undefined(cons, unresolved_mark) {
        return None;
    }

    // test must be `x === null || x === void 0`
    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(test, unresolved_mark)?;

    if let Some(real_rhs) = real_value {
        // Strict Babel lowering references the temp four times:
        // assignment target, null check left/right, and the final access/call.
        if is_standard_temp_expr(&checked, uninitialized_bindings, binding_references, 4)
            || is_generated_temp_expr(&checked, binding_references, 3)
            // Generated member/nested-chain temps skip the declaration-site proof, so we only
            // require the three references inside the lowered chain itself.
            || is_generated_member_temp_expr(&checked, &real_rhs, binding_references, 3)
            || is_nested_optional_chain_temp_expr(&checked, &real_rhs, binding_references, 3)
            || (is_optional_call_on_checked(alt, &checked)
                && (is_standard_temp_expr(
                    &checked,
                    uninitialized_bindings,
                    binding_references,
                    5,
                ) || is_generated_member_temp_expr(&checked, &real_rhs, binding_references, 5)))
        {
            if let Some(chain) =
                make_optional_chain_replacing(&checked, &real_rhs, alt, unresolved_mark)
            {
                return (policy.level >= RewriteLevel::Standard).then_some(chain);
            }
        }
        if policy.level < RewriteLevel::Aggressive {
            return None;
        }
        // Assignment form: `checked` is `tmp`, `real_rhs` is the original expr
        // alt must use `tmp` as the object
        return make_optional_chain_replacing(&checked, &real_rhs, alt, unresolved_mark);
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
        // x.prop.deep → x?.prop.deep
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            let replaced_obj = make_optional_chain(base, obj)?;
            Some(make_required_member_tail(replaced_obj, prop.clone()))
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

fn is_optional_call_on_checked(access: &Expr, checked: &Expr) -> bool {
    let Expr::OptChain(OptChainExpr { base, .. }) = strip_parens(access) else {
        return false;
    };
    let OptChainBase::Call(OptCall { callee, .. }) = base.as_ref() else {
        return false;
    };
    match strip_parens(callee.as_ref()) {
        Expr::Member(MemberExpr { obj, .. }) => exprs_structurally_equal(obj, checked),
        Expr::OptChain(OptChainExpr { base, .. }) => match base.as_ref() {
            OptChainBase::Member(MemberExpr { obj, .. }) => exprs_structurally_equal(obj, checked),
            OptChainBase::Call(_) => false,
        },
        _ => false,
    }
}

fn make_required_member_tail(obj: Expr, prop: MemberProp) -> Expr {
    if matches!(strip_parens(&obj), Expr::OptChain(_)) {
        return Expr::OptChain(OptChainExpr {
            span: DUMMY_SP,
            optional: false,
            base: Box::new(OptChainBase::Member(MemberExpr {
                span: DUMMY_SP,
                obj: Box::new(obj),
                prop,
            })),
        });
    }
    Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(obj),
        prop,
    })
}

/// Build an optional chain for the assignment temp-var case.
/// `tmp` is the temp variable expr, `real_rhs` is what it was assigned from.
/// `access` should use `tmp` as its object; we replace `tmp` with `real_rhs` in the output.
fn make_optional_chain_replacing(
    tmp: &Expr,
    real_rhs: &Expr,
    access: &Expr,
    unresolved_mark: Mark,
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
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            let replaced_obj = make_optional_chain_replacing(tmp, real_rhs, obj, unresolved_mark)?;
            Some(make_required_member_tail(replaced_obj, prop.clone()))
        }

        Expr::OptChain(OptChainExpr {
            base: opt_base,
            optional,
            ..
        }) => match opt_base.as_ref() {
            OptChainBase::Member(MemberExpr { obj, prop, .. }) => {
                let replaced_obj = if exprs_structurally_equal(obj, tmp) {
                    real_rhs.clone()
                } else {
                    make_optional_chain_replacing(tmp, real_rhs, obj, unresolved_mark)?
                };
                Some(Expr::OptChain(OptChainExpr {
                    span: DUMMY_SP,
                    optional: *optional,
                    base: Box::new(OptChainBase::Member(MemberExpr {
                        span: DUMMY_SP,
                        obj: Box::new(replaced_obj),
                        prop: prop.clone(),
                    })),
                }))
            }
            OptChainBase::Call(OptCall {
                callee,
                args,
                type_args,
                span,
                ctxt,
            }) => {
                let replaced_callee =
                    make_optional_chain_replacing(tmp, real_rhs, callee, unresolved_mark)?;
                Some(Expr::OptChain(OptChainExpr {
                    span: DUMMY_SP,
                    optional: *optional,
                    base: Box::new(OptChainBase::Call(OptCall {
                        span: *span,
                        ctxt: *ctxt,
                        callee: Box::new(replaced_callee),
                        args: args.clone(),
                        type_args: type_args.clone(),
                    })),
                }))
            }
        },

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
                            real_rhs,
                            args,
                            type_args,
                            *span,
                            *ctxt,
                            unresolved_mark,
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
    unresolved_mark: Mark,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
) -> Option<Expr> {
    if !policy.assumptions.no_document_all {
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
            if !is_void_or_undefined(cons, unresolved_mark) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            try_loose_chain_with_assign(
                checked,
                alt,
                policy,
                uninitialized_bindings,
                binding_references,
                unresolved_mark,
            )
        }
        // `x != null ? x.prop : undefined`
        // `(tmp = expr) != null ? tmp.prop : undefined`
        BinaryOp::NotEq => {
            if !is_void_or_undefined(alt, unresolved_mark) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            try_loose_chain_with_assign(
                checked,
                cons,
                policy,
                uninitialized_bindings,
                binding_references,
                unresolved_mark,
            )
        }
        _ => None,
    }
}

fn try_loose_chain_with_assign(
    checked: Expr,
    access: &Expr,
    policy: RewritePolicy,
    uninitialized_bindings: &HashSet<BindingId>,
    binding_references: &HashMap<BindingId, usize>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if let Some((tmp, real_rhs)) = extract_assign_parts(&checked) {
        let tmp_ident_expr = find_ident_by_binding(access, &tmp)?;
        let recovered_real_rhs = recover_lowered_optional_chain_expr(real_rhs, unresolved_mark)
            .map(|recovered| recovered.chain);
        // Loose Babel lowering references the temp twice inside the chain:
        // once in the null check and once in the final access/call.
        if is_standard_temp_expr(
            &tmp_ident_expr,
            uninitialized_bindings,
            binding_references,
            3,
        ) || is_generated_temp_expr(&tmp_ident_expr, binding_references, 2)
            || is_generated_member_temp_expr(&tmp_ident_expr, real_rhs, binding_references, 2)
            || is_nested_optional_chain_temp_expr(&tmp_ident_expr, real_rhs, binding_references, 2)
            || temp_expr_is_safe_for_pattern(
                &tmp_ident_expr,
                &[&checked, access],
                uninitialized_bindings,
                binding_references,
            )
        {
            if let Some(chain) = make_optional_chain_replacing_preferred(
                &tmp_ident_expr,
                real_rhs,
                recovered_real_rhs.as_ref(),
                access,
                unresolved_mark,
            ) {
                return Some(chain);
            }
        }
        if policy.assumptions.pure_getters {
            if let Some(recovered_access) =
                recover_loose_repeated_optional_call_chain(access, unresolved_mark)
            {
                if let Some(chain) = make_optional_chain_replacing(
                    &tmp_ident_expr,
                    real_rhs,
                    &recovered_access,
                    unresolved_mark,
                ) {
                    return Some(chain);
                }
            }
        }
        if policy.level < RewriteLevel::Aggressive {
            return None;
        }
        make_optional_chain_replacing_preferred(
            &tmp_ident_expr,
            real_rhs,
            recovered_real_rhs.as_ref(),
            access,
            unresolved_mark,
        )
    } else {
        make_optional_chain(checked, access)
    }
}

fn recover_loose_repeated_optional_call_chain(expr: &Expr, unresolved_mark: Mark) -> Option<Expr> {
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    if !is_void_or_undefined(cons, unresolved_mark) {
        return None;
    }
    let Expr::Bin(BinExpr {
        op: BinaryOp::EqEq,
        left,
        right,
        ..
    }) = test.as_ref()
    else {
        return None;
    };
    let checked = extract_loose_null_operand(left, right, unresolved_mark)?;

    if let Some((tmp, real_rhs)) = extract_assign_parts(&checked) {
        let tmp_ident_expr = find_ident_by_binding(alt, &tmp)?;
        let recovered_alt = recover_loose_repeated_optional_call_chain(alt, unresolved_mark)?;
        return make_optional_chain_replacing(
            &tmp_ident_expr,
            real_rhs,
            &recovered_alt,
            unresolved_mark,
        );
    }

    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee_expr),
        args,
        type_args,
        span,
        ctxt,
    }) = strip_parens(alt)
    else {
        return None;
    };
    if !exprs_structurally_equal(callee_expr.as_ref(), &checked) {
        return None;
    }

    Some(Expr::OptChain(OptChainExpr {
        span: DUMMY_SP,
        optional: true,
        base: Box::new(OptChainBase::Call(OptCall {
            span: *span,
            ctxt: *ctxt,
            callee: Box::new(checked),
            args: args.clone(),
            type_args: type_args.clone(),
        })),
    }))
}

fn make_optional_chain_replacing_preferred(
    tmp: &Expr,
    original_rhs: &Expr,
    recovered_rhs: Option<&Expr>,
    access: &Expr,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if is_call_expr(access) {
        return make_optional_chain_replacing(tmp, original_rhs, access, unresolved_mark).or_else(
            || make_optional_chain_replacing(tmp, recovered_rhs?, access, unresolved_mark),
        );
    }

    if let Some(recovered_access) = recover_lowered_optional_chain_expr(access, unresolved_mark)
        .map(|recovered| recovered.chain)
    {
        if let Some(chain) = recovered_rhs
            .and_then(|real_rhs| {
                make_optional_chain_replacing(tmp, real_rhs, &recovered_access, unresolved_mark)
            })
            .or_else(|| {
                make_optional_chain_replacing(tmp, original_rhs, &recovered_access, unresolved_mark)
            })
        {
            return Some(chain);
        }
    }

    recovered_rhs
        .and_then(|real_rhs| make_optional_chain_replacing(tmp, real_rhs, access, unresolved_mark))
        .or_else(|| make_optional_chain_replacing(tmp, original_rhs, access, unresolved_mark))
}

fn is_call_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Call(_))
}

fn find_ident_by_binding(access: &Expr, binding: &Ident) -> Option<Expr> {
    match access {
        Expr::Paren(paren) => find_ident_by_binding(&paren.expr, binding),
        Expr::Ident(id) if same_binding(id, binding) => Some(Expr::Ident(id.clone())),
        Expr::Cond(CondExpr {
            test, cons, alt, ..
        }) => find_ident_by_binding(test, binding)
            .or_else(|| find_ident_by_binding(cons, binding))
            .or_else(|| find_ident_by_binding(alt, binding)),
        Expr::Bin(BinExpr { left, right, .. }) => {
            find_ident_by_binding(left, binding).or_else(|| find_ident_by_binding(right, binding))
        }
        Expr::Assign(assign) => find_ident_by_binding(&assign.right, binding),
        Expr::Member(MemberExpr { obj, .. }) => {
            if let Expr::Ident(id) = &**obj {
                if same_binding(id, binding) {
                    return Some(Expr::Ident(id.clone()));
                }
            }
            find_ident_by_binding(obj, binding)
        }
        Expr::Call(CallExpr {
            callee: Callee::Expr(callee_expr),
            ..
        }) => {
            if let Expr::Member(MemberExpr { obj, .. }) = &**callee_expr {
                if let Expr::Ident(id) = &**obj {
                    if same_binding(id, binding) {
                        return Some(Expr::Ident(id.clone()));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn same_binding(a: &Ident, b: &Ident) -> bool {
    a.sym == b.sym && a.ctxt == b.ctxt
}

/// From a binary `x == null` or `null == x`, extract the non-null operand.
fn extract_loose_null_operand(
    left: &Box<Expr>,
    right: &Box<Expr>,
    unresolved_mark: Mark,
) -> Option<Expr> {
    if matches!(&**right, Expr::Lit(Lit::Null(_)))
        || is_unresolved_undefined(right, unresolved_mark)
    {
        return Some((**left).clone());
    }
    if matches!(&**left, Expr::Lit(Lit::Null(_))) || is_unresolved_undefined(left, unresolved_mark)
    {
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

    if let Some((tmp, real_rhs)) = extract_assign_parts(&left_val) {
        if let Expr::Ident(ri) = &*right_val {
            if same_binding(ri, &tmp) {
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

fn is_void_or_undefined(expr: &Expr, unresolved_mark: Mark) -> bool {
    is_unresolved_undefined(expr, unresolved_mark)
}

fn make_optional_call_replacing(
    real_rhs: &Expr,
    args: &[swc_core::ecma::ast::ExprOrSpread],
    type_args: &Option<Box<swc_core::ecma::ast::TsTypeParamInstantiation>>,
    span: swc_core::common::Span,
    ctxt: swc_core::common::SyntaxContext,
    unresolved_mark: Mark,
) -> Option<Expr> {
    let (context, call_args) = args.split_first()?;
    let recovered_callee =
        recover_babel_optional_call_callee(real_rhs, context.expr.as_ref(), unresolved_mark)?;
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

fn recover_babel_optional_call_callee(
    real_rhs: &Expr,
    context: &Expr,
    unresolved_mark: Mark,
) -> Option<Expr> {
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
                if recover_babel_call_context(obj, context).is_some() =>
            {
                Some(real_rhs.clone())
            }
            _ => None,
        },
        _ => {
            let recovered = recover_lowered_optional_chain_expr(real_rhs, unresolved_mark)?;
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

fn recover_lowered_optional_chain_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<RecoveredLoweredOptionalChain> {
    if let Some(recovered) = recover_strict_lowered_optional_chain_expr(expr, unresolved_mark) {
        return Some(recovered);
    }
    recover_loose_lowered_optional_chain_expr(expr, unresolved_mark)
}

fn recover_strict_lowered_optional_chain_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<RecoveredLoweredOptionalChain> {
    let Expr::Cond(CondExpr {
        test, cons, alt, ..
    }) = strip_parens(expr)
    else {
        return None;
    };

    if !is_void_or_undefined(cons, unresolved_mark) {
        return None;
    }

    let NullCheckResult {
        value: checked,
        real_value,
    } = extract_null_check(test, unresolved_mark)?;

    let expected_context = Some((*checked).clone());
    let chain = if let Some(real_rhs) = real_value {
        make_optional_chain_replacing(&checked, &real_rhs, alt, unresolved_mark)?
    } else {
        make_optional_chain(*checked, alt)?
    };

    Some(RecoveredLoweredOptionalChain {
        chain,
        expected_context,
    })
}

fn recover_loose_lowered_optional_chain_expr(
    expr: &Expr,
    unresolved_mark: Mark,
) -> Option<RecoveredLoweredOptionalChain> {
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
            if !is_void_or_undefined(cons, unresolved_mark) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            recover_loose_lowered_optional_chain_parts(checked, alt, unresolved_mark)
        }
        BinaryOp::NotEq => {
            if !is_void_or_undefined(alt, unresolved_mark) {
                return None;
            }
            let checked = extract_loose_null_operand(left, right, unresolved_mark)?;
            recover_loose_lowered_optional_chain_parts(checked, cons, unresolved_mark)
        }
        _ => None,
    }
}

fn recover_loose_lowered_optional_chain_parts(
    checked: Expr,
    access: &Expr,
    unresolved_mark: Mark,
) -> Option<RecoveredLoweredOptionalChain> {
    if let Some((tmp, real_rhs)) = extract_assign_parts(&checked) {
        let tmp_ident_expr = find_ident_by_binding(access, &tmp)?;
        Some(RecoveredLoweredOptionalChain {
            chain: make_optional_chain_replacing(
                &tmp_ident_expr,
                real_rhs,
                access,
                unresolved_mark,
            )?,
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
    let (assigned, assigned_rhs) = extract_assign_parts(member_obj)?;
    if same_binding(&assigned, context_ident) {
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

fn extract_assign_parts(expr: &Expr) -> Option<(Ident, &Box<Expr>)> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left else {
        return None;
    };
    Some((id.id.clone(), &assign.right))
}

fn extract_assign_ident_parts(expr: &Expr) -> Option<(Ident, &Box<Expr>)> {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(id)) = &assign.left else {
        return None;
    };
    Some((id.id.clone(), &assign.right))
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
    uninitialized_bindings.contains(&binding_id)
        && binding_references.get(&binding_id).copied() == Some(expected_references)
}

fn is_generated_temp_expr(
    checked: &Expr,
    binding_references: &HashMap<BindingId, usize>,
    expected_references: usize,
) -> bool {
    let Expr::Ident(Ident { sym, ctxt, .. }) = strip_parens(checked) else {
        return false;
    };
    let binding_id = (sym.clone(), *ctxt);
    looks_generated_temp_sym(sym)
        && binding_references.get(&binding_id).copied() == Some(expected_references)
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

fn is_babel_temp_sym(sym: &swc_core::atoms::Atom) -> bool {
    sym.starts_with('_')
}

fn looks_generated_temp_sym(sym: &swc_core::atoms::Atom) -> bool {
    is_babel_temp_sym(sym) || sym.chars().any(|ch| ch.is_ascii_digit())
}

#[derive(Default)]
struct BindingReferenceCounter {
    references: HashMap<BindingId, usize>,
}

impl Visit for BindingReferenceCounter {
    fn visit_ident(&mut self, ident: &Ident) {
        let binding_id = (ident.sym.clone(), ident.ctxt);
        *self.references.entry(binding_id).or_insert(0) += 1;
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{Globals, SyntaxContext, GLOBALS};
    use swc_core::ecma::ast::{AssignExpr, BindingIdent, ExprOrSpread, Null};

    use super::*;

    fn ident(sym: &str, ctxt: SyntaxContext) -> Ident {
        Ident::new(sym.into(), DUMMY_SP, ctxt)
    }

    fn ident_expr(sym: &str, ctxt: SyntaxContext) -> Expr {
        Expr::Ident(ident(sym, ctxt))
    }

    fn assign_ident_expr(sym: &str, ctxt: SyntaxContext, rhs: Expr) -> Expr {
        Expr::Assign(AssignExpr {
            span: DUMMY_SP,
            op: AssignOp::Assign,
            left: AssignTarget::Simple(SimpleAssignTarget::Ident(BindingIdent {
                id: ident(sym, ctxt),
                type_ann: None,
            })),
            right: Box::new(rhs),
        })
    }

    fn eq_null(expr: Expr) -> Expr {
        Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::EqEqEq,
            left: Box::new(expr),
            right: Box::new(Expr::Lit(Lit::Null(Null { span: DUMMY_SP }))),
        })
    }

    fn eq_undefined(expr: Expr, unresolved_mark: Mark) -> Expr {
        Expr::Bin(BinExpr {
            span: DUMMY_SP,
            op: BinaryOp::EqEqEq,
            left: Box::new(expr),
            right: Box::new(ident_expr(
                "undefined",
                SyntaxContext::empty().apply_mark(unresolved_mark),
            )),
        })
    }

    #[test]
    fn null_check_assignment_requires_matching_context() {
        GLOBALS.set(&Globals::default(), || {
            let unresolved_mark = Mark::new();
            let first_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let second_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let test = Expr::Bin(BinExpr {
                span: DUMMY_SP,
                op: BinaryOp::LogicalOr,
                left: Box::new(eq_null(assign_ident_expr(
                    "tmp",
                    first_ctxt,
                    ident_expr("obj", SyntaxContext::empty()),
                ))),
                right: Box::new(eq_undefined(
                    ident_expr("tmp", second_ctxt),
                    unresolved_mark,
                )),
            });

            assert!(extract_null_check(&test, unresolved_mark).is_none());
        });
    }

    #[test]
    fn babel_call_context_assignment_requires_matching_context() {
        GLOBALS.set(&Globals::default(), || {
            let first_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let second_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let member_obj =
                assign_ident_expr("tmp", first_ctxt, ident_expr("obj", SyntaxContext::empty()));
            let context = ident_expr("tmp", second_ctxt);

            assert!(recover_babel_call_context(&member_obj, &context).is_none());
        });
    }

    #[test]
    fn find_ident_by_binding_requires_matching_context() {
        GLOBALS.set(&Globals::default(), || {
            let first_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let second_ctxt = SyntaxContext::empty().apply_mark(Mark::new());
            let target = ident("tmp", first_ctxt);
            let access = Expr::Call(CallExpr {
                span: DUMMY_SP,
                ctxt: SyntaxContext::empty(),
                callee: Callee::Expr(Box::new(Expr::Member(MemberExpr {
                    span: DUMMY_SP,
                    obj: Box::new(ident_expr("tmp", second_ctxt)),
                    prop: MemberProp::Ident(ident("method", SyntaxContext::empty()).into()),
                }))),
                args: vec![ExprOrSpread {
                    spread: None,
                    expr: Box::new(ident_expr("arg", SyntaxContext::empty())),
                }],
                type_args: None,
            });

            assert!(find_ident_by_binding(&access, &target).is_none());
        });
    }
}
