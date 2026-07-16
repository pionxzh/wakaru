use std::collections::{HashMap, HashSet};

use swc_core::atoms::Atom;
use swc_core::common::{Mark, Span, Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrowExpr, AssignExpr, AssignOp, AssignTarget, AwaitExpr, BlockStmt, CallExpr, Callee, Expr,
    ExprOrSpread, ExprStmt, Function, Ident, IfStmt, MemberExpr, Module, Pat, Prop, PropName,
    ReturnStmt, SeqExpr, SimpleAssignTarget, Stmt, SwitchCase, VarDeclarator, YieldExpr,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::cross_module_helper_refs::{
    collect_cross_module_ts_helper_refs, cross_module_ts_member_helper,
};
use super::helper_matcher::{binding_key, ident_matches_binding};
use super::rename_utils::BindingId;
use super::state_machine::{
    invert_condition, stmts_contain_state_opcode_return, ForwardJumpJoin, IndexLoopContinueMode,
    OpcodeReturnScan, StateMachineProgram,
};
use super::transpiler_helper_utils::{BindingKey, LocalHelperContext, TsHelperKind};
use crate::facts::{ModuleFactsMap, TypeScriptHelperKind};
use crate::js_names::is_likely_generated_alias;
use crate::utils::paren::strip_parens;

pub struct UnAsyncAwait;

impl UnAsyncAwait {
    pub(crate) fn run_with_helpers(
        module: &mut swc_core::ecma::ast::Module,
        unresolved_mark: Mark,
        local_helpers: &LocalHelperContext,
        module_facts: Option<&ModuleFactsMap>,
        current_filename: Option<&str>,
    ) {
        let mut helpers =
            AsyncHelperContext::from_local_helpers(local_helpers, Some(unresolved_mark));
        if let Some(module_facts) = module_facts {
            helpers.extend_cross_module(module, module_facts, current_filename);
        }
        module.visit_mut_with(&mut UnAsyncAwaitWithHelpers { helpers: &helpers });
        module.visit_mut_with(&mut AwaiterIifeTransformer { helpers: &helpers });
        remove_unused_inline_async_helpers(module, local_helpers);
    }
}

impl VisitMut for UnAsyncAwait {
    fn visit_mut_module(&mut self, module: &mut Module) {
        let local_helpers = LocalHelperContext::collect(module);
        let helpers = AsyncHelperContext::from_local_helpers(&local_helpers, None);
        module.visit_mut_with(&mut UnAsyncAwaitWithHelpers { helpers: &helpers });
        module.visit_mut_with(&mut AwaiterIifeTransformer { helpers: &helpers });
        remove_unused_inline_async_helpers(module, &local_helpers);
    }

    fn visit_mut_function(&mut self, func: &mut Function) {
        let helpers = AsyncHelperContext::default();
        visit_mut_function_with_helpers(func, &helpers);
    }
}

struct UnAsyncAwaitWithHelpers<'a> {
    helpers: &'a AsyncHelperContext,
}

impl VisitMut for UnAsyncAwaitWithHelpers<'_> {
    fn visit_mut_function(&mut self, func: &mut Function) {
        visit_mut_function_with_helpers(func, self.helpers);
    }
}

struct AwaiterIifeTransformer<'a> {
    helpers: &'a AsyncHelperContext,
}

impl VisitMut for AwaiterIifeTransformer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        try_transform_awaiter_iife(expr, self.helpers);
    }
}

#[derive(Default)]
struct AsyncHelperContext {
    awaiter_helpers: HashSet<BindingKey>,
    awaiter_namespaces: HashMap<BindingKey, HashSet<String>>,
    generator_helpers: HashSet<BindingKey>,
    generator_namespaces: HashMap<BindingKey, HashSet<String>>,
    values_helpers: HashSet<BindingKey>,
    unresolved_mark: Option<Mark>,
}

impl AsyncHelperContext {
    fn from_local_helpers(
        local_helpers: &LocalHelperContext,
        unresolved_mark: Option<Mark>,
    ) -> Self {
        Self {
            awaiter_helpers: local_helpers.ts_helpers_of_kind(TsHelperKind::Awaiter),
            awaiter_namespaces: HashMap::new(),
            generator_helpers: local_helpers.ts_helpers_of_kind(TsHelperKind::Generator),
            generator_namespaces: HashMap::new(),
            values_helpers: local_helpers.ts_helpers_of_kind(TsHelperKind::Values),
            unresolved_mark,
        }
    }

    fn extend_cross_module(
        &mut self,
        module: &Module,
        module_facts: &ModuleFactsMap,
        current_filename: Option<&str>,
    ) {
        let awaiter = collect_cross_module_ts_helper_refs(
            module,
            module_facts,
            current_filename,
            TypeScriptHelperKind::Awaiter,
        );
        self.awaiter_helpers.extend(awaiter.direct);
        self.awaiter_namespaces.extend(awaiter.namespaces);

        let generator = collect_cross_module_ts_helper_refs(
            module,
            module_facts,
            current_filename,
            TypeScriptHelperKind::Generator,
        );
        self.generator_helpers.extend(generator.direct);
        self.generator_namespaces.extend(generator.namespaces);
    }

    fn is_awaiter_call(&self, call: &swc_core::ecma::ast::CallExpr) -> bool {
        self.matches_helper_call(
            call,
            &self.awaiter_helpers,
            &self.awaiter_namespaces,
            "__awaiter",
        )
    }

    fn is_generator_call(&self, call: &swc_core::ecma::ast::CallExpr) -> bool {
        self.matches_helper_call(
            call,
            &self.generator_helpers,
            &self.generator_namespaces,
            "__generator",
        )
    }

    fn matches_helper_call(
        &self,
        call: &swc_core::ecma::ast::CallExpr,
        helpers: &HashSet<BindingKey>,
        namespaces: &HashMap<BindingKey, HashSet<String>>,
        canonical_name: &str,
    ) -> bool {
        let Some(callee) = call
            .callee
            .as_expr()
            .map(|expr| strip_parens(expr.as_ref()))
        else {
            return false;
        };
        match callee {
            Expr::Ident(id) => {
                helpers.contains(&binding_key(id))
                    || (id.sym.as_ref() == canonical_name
                        && self
                            .unresolved_mark
                            .is_some_and(|mark| id.ctxt.outer() == mark))
            }
            Expr::Member(_) => cross_module_ts_member_helper(callee, namespaces),
            _ => false,
        }
    }
}

fn visit_mut_function_with_helpers(func: &mut Function, helpers: &AsyncHelperContext) {
    let awaiter_param_hints = collect_awaiter_param_hints(func, helpers);

    // Recurse into children first
    func.visit_mut_children_with(&mut UnAsyncAwaitWithHelpers { helpers });

    let body = match func.body.as_mut() {
        Some(b) => b,
        None => return,
    };

    // Try __generator transform first (makes function a generator)
    if try_transform_generator(body, helpers) {
        func.is_generator = true;
        return;
    }

    // Try __awaiter transform (makes function async).
    // After extracting the inner body, also run the generator transform
    // in case the inner function was a __generator state machine.
    let saved_stmts = body.stmts.clone();
    if try_transform_awaiter(body, helpers) {
        try_transform_generator(body, helpers);
        if stmts_contain_state_opcode_return(&body.stmts, OpcodeReturnScan::SkipNestedFunctions)
            || contains_unresolved_generator_wrapper(body, helpers)
        {
            body.stmts = saved_stmts;
        } else {
            func.is_async = true;
            apply_unused_param_hints(func, awaiter_param_hints);
        }
    }
}

/// An awaiter wrapper is only safe to remove when its generator wrapper in the
/// current function body was decoded too. Otherwise the async function would
/// return the generator iterator itself instead of awaiting the state machine's
/// result. Independent nested callables are transformed on their own and must
/// not make the containing awaiter roll back.
fn contains_unresolved_generator_wrapper(body: &BlockStmt, helpers: &AsyncHelperContext) -> bool {
    struct Finder<'a> {
        helpers: &'a AsyncHelperContext,
        found: bool,
    }

    impl Visit for Finder<'_> {
        fn visit_function(&mut self, _function: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.helpers.is_generator_call(call)
                && call
                    .args
                    .get(1)
                    .is_some_and(|arg| matches!(strip_parens(&arg.expr), Expr::Fn(_)))
            {
                self.found = true;
                return;
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder {
        helpers,
        found: false,
    };
    body.visit_with(&mut finder);
    finder.found
}

// ============================================================
// __generator state-machine -> function*
// ============================================================

pub(crate) fn try_transform_ts_generator_body(
    body: &mut BlockStmt,
    generator_helpers: &[BindingKey],
) -> bool {
    let helpers = AsyncHelperContext {
        awaiter_helpers: HashSet::new(),
        awaiter_namespaces: HashMap::new(),
        generator_helpers: generator_helpers.iter().cloned().collect(),
        generator_namespaces: HashMap::new(),
        values_helpers: HashSet::new(),
        unresolved_mark: None,
    };
    try_transform_generator(body, &helpers)
}

fn try_transform_generator(body: &mut BlockStmt, helpers: &AsyncHelperContext) -> bool {
    // Find: return __generator(this, function(_a) { switch(_a.label) { ... } })
    let return_idx = body
        .stmts
        .iter()
        .position(|stmt| is_generator_return(stmt, helpers));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    let new_stmts = match extract_generator_stmts(body.stmts[return_idx].clone(), helpers) {
        Some(stmts) => stmts,
        None => return false,
    };
    body.stmts.remove(return_idx);

    // Insert new statements where the return was
    body.stmts.splice(return_idx..return_idx, new_stmts);
    true
}

fn is_generator_return(stmt: &Stmt, helpers: &AsyncHelperContext) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    helpers.is_generator_call(call)
}

fn extract_generator_stmts(stmt: Stmt, helpers: &AsyncHelperContext) -> Option<Vec<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(mut call) = arg else {
        return None;
    };
    if !helpers.is_generator_call(&call) {
        return None;
    }
    if call.args.len() < 2 {
        return None;
    }

    let fn_arg = *call.args.remove(1).expr;
    let Expr::Fn(fn_expr) = fn_arg else {
        return None;
    };
    let state_name: Atom = fn_expr.function.params.first().and_then(|p| {
        if let Pat::Ident(bi) = &p.pat {
            Some(bi.id.sym.clone())
        } else {
            None
        }
    })?;
    let body = fn_expr.function.body?;
    let mut stmts = body.stmts.into_iter();
    let first = stmts.next()?;
    if let Stmt::Switch(sw) = first {
        return decode_state_machine(state_name, sw.cases, &helpers.values_helpers);
    }
    if stmts.next().is_none() {
        if let Some(decoded) = decode_return_opcode(&first, &helpers.values_helpers) {
            return Some(decoded.into_iter().collect());
        }
    }
    None
}

/// Expand Terser-compressed case body statements back into the form the
/// decoder expects. Terser merges individual statements into comma sequences
/// and folds conditional branches into ternary returns:
///
///   `a, b, _a.label = 1;`    ->  `a; b; _a.label = 1;`
///   `return index++, [3, 1]` ->  `index++; return [3, 1];`
///   `return t ? [4, X] : [3, N]` ->  `if (!t) return [3, N]; return [4, X];`
fn expand_terser_case_stmts(stmts: &[Stmt]) -> Vec<Stmt> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < stmts.len() {
        if let Some(consumed) = try_rearrange_if_goto_pair(&stmts[i..], &mut result) {
            i += consumed;
        } else {
            expand_one_stmt(&mut result, &stmts[i]);
            i += 1;
        }
    }
    result
}

/// Rearrange `if (test) { return [opcode, X]; } return [3, N];` into
/// `if (!test) return [3, N]; return [opcode, X];` so the goto ends up in
/// the if-body where `loop_break_test` can detect it. `SimplifySequence`
/// produces this pattern from `return test ? [opcode, X] : [3, N]`.
fn try_rearrange_if_goto_pair(stmts: &[Stmt], result: &mut Vec<Stmt>) -> Option<usize> {
    if stmts.len() < 2 {
        return None;
    }
    let Stmt::If(if_stmt) = &stmts[0] else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    if !if_body_has_opcode_return(&if_stmt.cons) {
        return None;
    }
    let Stmt::Return(ret) = &stmts[1] else {
        return None;
    };
    if !is_goto_opcode_return(ret) {
        return None;
    }
    result.push(Stmt::If(IfStmt {
        span: if_stmt.span,
        test: invert_condition(&if_stmt.test),
        cons: Box::new(stmts[1].clone()),
        alt: None,
    }));
    flatten_if_cons_into(result, &if_stmt.cons);
    Some(2)
}

fn if_body_has_opcode_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(ret) => ret
            .arg
            .as_deref()
            .is_some_and(|a| is_opcode_like(strip_parens(a))),
        Stmt::Block(block) if block.stmts.len() == 1 => if_body_has_opcode_return(&block.stmts[0]),
        _ => false,
    }
}

fn is_goto_opcode_return(ret: &ReturnStmt) -> bool {
    let Some(arg) = ret.arg.as_deref() else {
        return false;
    };
    let Expr::Array(arr) = strip_parens(arg) else {
        return false;
    };
    arr.elems.first().and_then(|e| e.as_ref()).is_some_and(|e| {
        matches!(
            e.expr.as_ref(),
            Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) if n.value as u32 == 3
        )
    })
}

fn flatten_if_cons_into(result: &mut Vec<Stmt>, stmt: &Stmt) {
    match stmt {
        Stmt::Block(block) => {
            for s in &block.stmts {
                expand_one_stmt(result, s);
            }
        }
        _ => expand_one_stmt(result, stmt),
    }
}

fn expand_one_stmt(result: &mut Vec<Stmt>, stmt: &Stmt) {
    if let Stmt::Return(ret) = stmt {
        if let Some(Expr::Paren(paren)) = ret.arg.as_deref() {
            return expand_one_stmt(
                result,
                &Stmt::Return(ReturnStmt {
                    span: ret.span,
                    arg: Some(paren.expr.clone()),
                }),
            );
        }
    }
    match stmt {
        Stmt::Expr(ExprStmt { span, expr, .. }) if matches!(expr.as_ref(), Expr::Seq(_)) => {
            let Expr::Seq(seq) = expr.as_ref() else {
                unreachable!()
            };
            for sub in &seq.exprs {
                result.push(Stmt::Expr(ExprStmt {
                    span: *span,
                    expr: sub.clone(),
                }));
            }
        }
        Stmt::Return(ret)
            if ret
                .arg
                .as_deref()
                .is_some_and(|a| matches!(a, Expr::Seq(_))) =>
        {
            let Expr::Seq(seq) = ret.arg.as_ref().unwrap().as_ref() else {
                unreachable!()
            };
            for sub in &seq.exprs[..seq.exprs.len() - 1] {
                result.push(Stmt::Expr(ExprStmt {
                    span: ret.span,
                    expr: sub.clone(),
                }));
            }
            expand_one_stmt(
                result,
                &Stmt::Return(ReturnStmt {
                    span: ret.span,
                    arg: Some(seq.exprs.last().unwrap().clone()),
                }),
            );
        }
        Stmt::Return(ret)
            if ret
                .arg
                .as_deref()
                .is_some_and(|a| matches!(a, Expr::Cond(_))) =>
        {
            let Expr::Cond(cond) = ret.arg.as_ref().unwrap().as_ref() else {
                unreachable!()
            };
            if is_opcode_like(&cond.cons) || is_opcode_like(&cond.alt) {
                result.push(Stmt::If(IfStmt {
                    span: ret.span,
                    test: invert_condition(&cond.test),
                    cons: Box::new(Stmt::Return(ReturnStmt {
                        span: ret.span,
                        arg: Some(cond.alt.clone()),
                    })),
                    alt: None,
                }));
                expand_one_stmt(
                    result,
                    &Stmt::Return(ReturnStmt {
                        span: ret.span,
                        arg: Some(cond.cons.clone()),
                    }),
                );
            } else {
                result.push(stmt.clone());
            }
        }
        _ => result.push(stmt.clone()),
    }
}

fn is_opcode_like(expr: &Expr) -> bool {
    let inner = unwrap_seq_last(strip_parens(expr));
    if let Expr::Array(arr) = inner {
        arr.elems
            .first()
            .and_then(|e| e.as_ref())
            .is_some_and(|e| matches!(e.expr.as_ref(), Expr::Lit(swc_core::ecma::ast::Lit::Num(_))))
    } else {
        false
    }
}

fn unwrap_seq_last(expr: &Expr) -> &Expr {
    let expr = strip_parens(expr);
    if let Expr::Seq(SeqExpr { exprs, .. }) = expr {
        exprs
            .last()
            .map(|e| strip_parens(e.as_ref()))
            .unwrap_or(expr)
    } else {
        expr
    }
}

/// Decode the state machine into a flat list of statements.
///
/// Phase 1: Collect (label_idx, Stmt) pairs in case order, decoding opcodes.
/// Phase 2: Merge `_a.sent()` usages with the previous yield:
///   - standalone `_a.sent();` -> drop
///   - `v = _a.sent()` -> pop prev `yield X;`, push `v = yield X;`
///
/// Phase 3: Group by label and reconstruct try/catch/finally blocks.
fn decode_state_machine(
    state_name: Atom,
    cases: Vec<SwitchCase>,
    values_helpers: &HashSet<BindingKey>,
) -> Option<Vec<Stmt>> {
    let mut trys: Vec<[Option<usize>; 4]> = Vec::new();
    // (label_idx, stmt) pairs
    let mut flat: Vec<(usize, Stmt)> = Vec::new();

    for case in &cases {
        let idx = match numeric_case_test(case) {
            Some(n) => n as usize,
            None => continue,
        };
        let next_case_label = next_numeric_case_label(&cases, idx);

        let expanded = expand_terser_case_stmts(&case.cons);
        for stmt in &expanded {
            if let Some(region) = extract_trys_push(&state_name, stmt) {
                trys.push(region);
                continue;
            }
            if is_state_label_assign(&state_name, stmt) {
                continue;
            }

            if let Some(decoded) = decode_return_opcode_with_backedge(
                stmt,
                values_helpers,
                idx,
                next_case_label,
                &trys,
            ) {
                if let Some(s) = decoded {
                    flat.push((idx, s));
                }
                continue;
            }

            flat.push((idx, stmt.clone()));
        }
    }

    // Phase 2: merge _a.sent() with previous yield
    let mut output: Vec<(usize, Stmt)> = Vec::new();
    // Catch-region temps. TSC lowers `catch (error)` to a function-scoped temp
    // assigned from `_a.sent()` (`error_1 = _a.sent(); use(error_1)`). We fold
    // that alias back into the synthesized `error` catch binding.
    let mut catch_aliases: Vec<BindingKey> = Vec::new();
    for (idx, stmt) in flat {
        if is_standalone_sent(&state_name, &stmt) {
            // Standalone _a.sent(); -- the caller discards the yielded value. Drop.
            continue;
        }
        let in_catch = is_catch_label(idx, &trys);
        if !in_catch {
            catch_aliases.clear();
        }
        if in_catch {
            // `error_1 = _a.sent()` aliases the caught value. Record it and drop
            // the assignment; later references resolve to the `error` binding.
            if let Some(alias) = catch_sent_alias(&state_name, &stmt) {
                catch_aliases.push(alias);
                continue;
            }
            // Rewrite both `_a.sent()` and any recorded alias to `error`.
            let mut replacer = CatchValueReplacer {
                state_name: state_name.clone(),
                aliases: catch_aliases.clone(),
                replacement: Box::new(Expr::Ident(Ident::new_no_ctxt("error".into(), DUMMY_SP))),
            };
            let mut s = stmt;
            s.visit_mut_with(&mut replacer);
            output.push((idx, s));
            continue;
        }
        if stmt_uses_sent(&state_name, &stmt) {
            if let Some((_, prev)) = output.last() {
                if let Some(split) = split_sent_consuming_stmt(&state_name, &stmt, prev) {
                    output.pop();
                    output.extend(split.into_iter().map(|stmt| (idx, stmt)));
                    continue;
                }
            }
            // Pop the previous yield and embed it into this assignment/expression.
            let merged = if let Some((_, prev)) = output.last() {
                extract_yield_from_stmt(prev).map(|(arg, delegate, yield_span)| {
                    let yield_expr = Box::new(Expr::Yield(YieldExpr {
                        span: yield_span,
                        delegate,
                        arg: Some(arg),
                    }));
                    let mut replacer = SentReplacer {
                        state_name: state_name.clone(),
                        replacement: yield_expr,
                    };
                    let mut s = stmt.clone();
                    s.visit_mut_with(&mut replacer);
                    s
                })
            } else {
                None
            };
            if let Some(merged_stmt) = merged {
                output.pop();
                output.push((idx, merged_stmt));
            } else {
                // No previous yield -- replace sent with undefined
                let mut replacer = SentReplacer {
                    state_name: state_name.clone(),
                    replacement: Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    ))),
                };
                let mut s = stmt;
                s.visit_mut_with(&mut replacer);
                output.push((idx, s));
            }
        } else {
            output.push((idx, stmt));
        }
    }

    let mut recovered = StateMachineProgram::from_labeled_stmts(output, trys)
        .recover_conditional_assignments()
        .recover_conditional_branches(OpcodeReturnScan::SkipNestedFunctions)
        .resolve_labeled_forward_jumps(
            OpcodeReturnScan::SkipNestedFunctions,
            ForwardJumpJoin::MidMachine,
        )
        .into_reconstructed_stmts_with_index_loops(IndexLoopContinueMode::AdjacentBackEdge);
    fold_memoized_member_apply_calls(&mut recovered);
    if stmts_contain_state_opcode_return(&recovered, OpcodeReturnScan::SkipNestedFunctions) {
        return None;
    }
    Some(recovered)
}

fn is_catch_label(label_idx: usize, trys: &[[Option<usize>; 4]]) -> bool {
    trys.iter().any(|region| region[1] == Some(label_idx))
}

/// If `stmt` is `ExprStmt(yield X)`, return `(X, delegate, yield_span)`.
fn extract_yield_from_stmt(stmt: &Stmt) -> Option<(Box<Expr>, bool, Span)> {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Yield(y) = expr.as_ref() {
            let arg = y.arg.clone().unwrap_or_else(|| {
                Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "undefined".into(),
                    DUMMY_SP,
                )))
            });
            return Some((arg, y.delegate, y.span));
        }
    }
    None
}

fn split_sent_consuming_stmt(state_name: &Atom, stmt: &Stmt, prev: &Stmt) -> Option<Vec<Stmt>> {
    let (arg, delegate, yield_span) = extract_yield_from_stmt(prev)?;
    let stmt_span = stmt.span();
    let yielded = Box::new(Expr::Yield(YieldExpr {
        span: yield_span,
        delegate,
        arg: Some(arg),
    }));

    if let Some((left, followup)) = split_yield_arg_sent_assignment(state_name, stmt) {
        return Some(vec![
            assign_stmt(left, yielded, stmt_span),
            Stmt::Expr(ExprStmt {
                span: stmt_span,
                expr: Box::new(Expr::Yield(YieldExpr {
                    span: stmt_span,
                    delegate: false,
                    arg: Some(followup),
                })),
            }),
        ]);
    }

    if let Some((left, returned)) = split_return_sent_assignment(state_name, stmt) {
        return Some(vec![
            assign_stmt(left, yielded, stmt_span),
            Stmt::Return(swc_core::ecma::ast::ReturnStmt {
                span: stmt_span,
                arg: Some(returned),
            }),
        ]);
    }

    None
}

fn split_yield_arg_sent_assignment(
    state_name: &Atom,
    stmt: &Stmt,
) -> Option<(AssignTarget, Box<Expr>)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Yield(yield_expr) = expr.as_ref() else {
        return None;
    };
    if yield_expr.delegate {
        return None;
    }
    let arg = yield_expr.arg.as_deref()?;
    let Expr::Call(call) = arg else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(member) = callee.as_ref() else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(&member.obj) else {
        return None;
    };
    if assign.op != AssignOp::Assign || !is_sent_call(state_name, &assign.right) {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };

    let mut next_member: MemberExpr = member.clone();
    next_member.obj = Box::new(Expr::Ident(left.id.clone()));
    let mut next_call = call.clone();
    next_call.callee = swc_core::ecma::ast::Callee::Expr(Box::new(Expr::Member(next_member)));

    Some((assign.left.clone(), Box::new(Expr::Call(next_call))))
}

fn split_return_sent_assignment(
    state_name: &Atom,
    stmt: &Stmt,
) -> Option<(AssignTarget, Box<Expr>)> {
    let Stmt::Return(ret) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = ret.arg.as_deref()? else {
        return None;
    };
    if assign.op != AssignOp::Assign || !is_sent_call(state_name, &assign.right) {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left else {
        return None;
    };
    Some((assign.left.clone(), Box::new(Expr::Ident(left.id.clone()))))
}

fn assign_stmt(left: AssignTarget, right: Box<Expr>, span: Span) -> Stmt {
    Stmt::Expr(ExprStmt {
        span,
        expr: Box::new(Expr::Assign(AssignExpr {
            span,
            op: AssignOp::Assign,
            left,
            right,
        })),
    })
}

fn is_sent_call(state_name: &Atom, expr: &Expr) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) else {
        return false;
    };
    matches!(mem.obj.as_ref(), Expr::Ident(id) if id.sym == *state_name)
        && is_ident_prop(&mem.prop, "sent")
}

/// Match `ident = _a.sent()` inside a catch region, returning the aliased
/// local binding. TSC stores the caught value in a function-scoped temp before
/// using it; we fold that temp into the reconstructed catch binding.
fn catch_sent_alias(state_name: &Atom, stmt: &Stmt) -> Option<BindingKey> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign || !is_sent_call(state_name, &assign.right) {
        return None;
    }
    let ident = assign.left.as_simple()?.as_ident()?;
    Some(binding_key(&ident.id))
}

fn is_standalone_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    if let Stmt::Expr(ExprStmt { expr, .. }) = stmt {
        if let Expr::Call(call) = expr.as_ref() {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == *state_name && is_ident_prop(&mem.prop, "sent") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn numeric_case_test(case: &SwitchCase) -> Option<f64> {
    let test = case.test.as_ref()?;
    if let Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) = test.as_ref() {
        Some(n.value)
    } else {
        None
    }
}

fn next_numeric_case_label(cases: &[SwitchCase], current: usize) -> Option<usize> {
    cases
        .iter()
        .filter_map(|case| numeric_case_test(case).map(|n| n as usize))
        .filter(|label| *label > current)
        .min()
}

fn extract_trys_push(state_name: &Atom, stmt: &Stmt) -> Option<[Option<usize>; 4]> {
    // _a.trys.push([s, c, f, n])
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let Expr::Member(callee_mem) = &**call.callee.as_expr()? else {
        return None;
    };
    let Expr::Member(outer_mem) = callee_mem.obj.as_ref() else {
        return None;
    };
    let Expr::Ident(obj_id) = outer_mem.obj.as_ref() else {
        return None;
    };
    if obj_id.sym != *state_name {
        return None;
    }
    if !is_ident_prop(&outer_mem.prop, "trys") {
        return None;
    }
    if !is_ident_prop(&callee_mem.prop, "push") {
        return None;
    }
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Array(arr) = call.args[0].expr.as_ref() else {
        return None;
    };
    if arr.elems.len() != 4 {
        return None;
    }
    let region: [Option<usize>; 4] = std::array::from_fn(|i| {
        arr.elems[i].as_ref().and_then(|e| {
            if let Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) = e.expr.as_ref() {
                Some(n.value as usize)
            } else {
                None
            }
        })
    });
    Some(region)
}

fn is_try_region_exit(label: usize, target: usize, trys: &[[Option<usize>; 4]]) -> bool {
    trys.iter().any(|region| {
        let Some(start) = region[0] else {
            return false;
        };
        let Some(end) = region[3].or(region[2]).or(region[1]) else {
            return false;
        };
        if label < start || label >= end {
            return false;
        }
        region[3].is_some_and(|next| target == next)
            || region[2].is_some_and(|finally_start| target == finally_start)
    })
}

fn is_state_label_assign(state_name: &Atom, stmt: &Stmt) -> bool {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return false;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return false;
    };
    let Some(left_expr) = assign.left.as_simple().and_then(|s| s.as_member()) else {
        return false;
    };
    let Expr::Ident(id) = left_expr.obj.as_ref() else {
        return false;
    };
    id.sym == *state_name && is_ident_prop(&left_expr.prop, "label")
}

/// Returns `Some(Some(stmt))` if an opcode-based return was decoded,
/// `Some(None)` to drop the statement, or `None` if not a return opcode.
fn decode_return_opcode(stmt: &Stmt, values_helpers: &HashSet<BindingKey>) -> Option<Option<Stmt>> {
    decode_return_opcode_with_backedge(stmt, values_helpers, 0, None, &[])
}

/// Like `decode_return_opcode`, but preserves non-fallthrough goto opcodes so
/// shared state-machine recovery can reconstruct loops and structured branches.
fn decode_return_opcode_with_backedge(
    stmt: &Stmt,
    values_helpers: &HashSet<BindingKey>,
    current_case: usize,
    next_case_label: Option<usize>,
    trys: &[[Option<usize>; 4]],
) -> Option<Option<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = ret.arg.as_ref()?;
    let Expr::Array(arr) = arg.as_ref() else {
        return None;
    };
    if arr.elems.is_empty() {
        return None;
    }
    let opcode = match arr.elems[0].as_ref()?.expr.as_ref() {
        Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) => n.value as u32,
        _ => return None,
    };
    let argument = arr
        .elems
        .get(1)
        .and_then(|e| e.as_ref())
        .map(|e| e.expr.clone());

    let ret_span = ret.span;

    match opcode {
        2 => {
            // return(value?)
            let s = argument.map(|a| {
                Stmt::Return(swc_core::ecma::ast::ReturnStmt {
                    span: ret_span,
                    arg: Some(a),
                })
            });
            Some(s)
        }
        3 => {
            // goto(label) -- preserve back-edges for loop recovery and
            // non-fallthrough forward jumps that mark if/else joins.
            if let Some(target) = argument.as_deref().and_then(|e| {
                if let Expr::Lit(swc_core::ecma::ast::Lit::Num(n)) = e {
                    Some(n.value as usize)
                } else {
                    None
                }
            }) {
                if target > 0
                    && (target < current_case
                        || (target > current_case && Some(target) != next_case_label))
                    && !is_try_region_exit(current_case, target, trys)
                {
                    return Some(Some(stmt.clone()));
                }
            }
            Some(None)
        }
        4 => {
            // yield(value)
            let expr = argument.unwrap_or_else(|| {
                Box::new(Expr::Ident(Ident::new_no_ctxt(
                    "undefined".into(),
                    DUMMY_SP,
                )))
            });
            Some(Some(Stmt::Expr(ExprStmt {
                span: ret_span,
                expr: Box::new(Expr::Yield(YieldExpr {
                    span: ret_span,
                    delegate: false,
                    arg: Some(expr),
                })),
            })))
        }
        5 => {
            // yield*(value)
            let expr = argument
                .map(|a| unwrap_ts_values(a, values_helpers))
                .unwrap_or_else(|| {
                    Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    )))
                });
            Some(Some(Stmt::Expr(ExprStmt {
                span: ret_span,
                expr: Box::new(Expr::Yield(YieldExpr {
                    span: ret_span,
                    delegate: true,
                    arg: Some(expr),
                })),
            })))
        }
        0 | 1 | 6 | 7 => Some(None), // skip
        _ => Some(Some(stmt.clone())),
    }
}

fn unwrap_ts_values(expr: Box<Expr>, values_helpers: &HashSet<BindingKey>) -> Box<Expr> {
    let Expr::Call(call) = expr.as_ref() else {
        return expr;
    };
    let Some(callee) = call.callee.as_expr() else {
        return expr;
    };
    let Expr::Ident(id) = callee.as_ref() else {
        return expr;
    };
    // Match either a detected `__values` / `_ts_values` binding (robust to
    // minified aliases) or the canonical helper names.
    let is_values_helper = values_helpers.contains(&binding_key(id))
        || matches!(id.sym.as_ref(), "__values" | "_ts_values");
    if is_values_helper {
        return call
            .args
            .first()
            .map(|arg| arg.expr.clone())
            .unwrap_or(expr);
    }
    expr
}

fn stmt_uses_sent(state_name: &Atom, stmt: &Stmt) -> bool {
    struct Finder {
        state_name: Atom,
        found: bool,
    }
    impl swc_core::ecma::visit::Visit for Finder {
        fn visit_function(&mut self, _func: &Function) {}

        fn visit_arrow_expr(&mut self, _arrow: &ArrowExpr) {}

        fn visit_call_expr(&mut self, call: &swc_core::ecma::ast::CallExpr) {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == self.state_name && is_ident_prop(&mem.prop, "sent") {
                        self.found = true;
                        return;
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut f = Finder {
        state_name: state_name.clone(),
        found: false,
    };
    swc_core::ecma::visit::VisitWith::visit_with(stmt, &mut f);
    f.found
}

struct SentReplacer {
    state_name: Atom,
    replacement: Box<Expr>,
}

impl VisitMut for SentReplacer {
    fn visit_mut_function(&mut self, _func: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == self.state_name && is_ident_prop(&mem.prop, "sent") {
                        *expr = *self.replacement.clone();
                        return;
                    }
                }
            }
        }
        expr.visit_mut_children_with(self);
    }
}

/// Replaces `_a.sent()` and any recorded catch-temp aliases with the catch
/// binding inside a reconstructed catch body.
struct CatchValueReplacer {
    state_name: Atom,
    aliases: Vec<BindingKey>,
    replacement: Box<Expr>,
}

impl VisitMut for CatchValueReplacer {
    fn visit_mut_function(&mut self, _func: &mut Function) {}

    fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if let Some(mem) = call.callee.as_expr().and_then(|e| e.as_member()) {
                if let Expr::Ident(id) = mem.obj.as_ref() {
                    if id.sym == self.state_name && is_ident_prop(&mem.prop, "sent") {
                        *expr = *self.replacement.clone();
                        return;
                    }
                }
            }
        }
        if let Expr::Ident(id) = expr {
            if self
                .aliases
                .iter()
                .any(|key| ident_matches_binding(id, key))
            {
                *expr = *self.replacement.clone();
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}

fn fold_memoized_member_apply_calls(stmts: &mut Vec<Stmt>) {
    let mut folded = Vec::new();
    let mut index = 0usize;

    while index < stmts.len() {
        if let Some((stmt, consumed)) = try_fold_memoized_member_apply_call(&stmts[index..]) {
            folded.push(stmt);
            index += consumed;
        } else {
            let mut stmt = stmts[index].clone();
            fold_memoized_member_apply_calls_in_stmt(&mut stmt);
            folded.push(stmt);
            index += 1;
        }
    }

    *stmts = folded;
}

fn fold_memoized_member_apply_calls_in_stmt(stmt: &mut Stmt) {
    match stmt {
        Stmt::Block(block) => fold_memoized_member_apply_calls(&mut block.stmts),
        Stmt::For(for_stmt) => {
            if let Stmt::Block(block) = for_stmt.body.as_mut() {
                fold_memoized_member_apply_calls(&mut block.stmts);
            }
        }
        Stmt::Try(try_stmt) => {
            fold_memoized_member_apply_calls(&mut try_stmt.block.stmts);
            if let Some(handler) = &mut try_stmt.handler {
                fold_memoized_member_apply_calls(&mut handler.body.stmts);
            }
            if let Some(finalizer) = &mut try_stmt.finalizer {
                fold_memoized_member_apply_calls(&mut finalizer.stmts);
            }
        }
        Stmt::If(if_stmt) => {
            fold_memoized_member_apply_calls_in_stmt(&mut if_stmt.cons);
            if let Some(alt) = &mut if_stmt.alt {
                fold_memoized_member_apply_calls_in_stmt(alt);
            }
        }
        _ => {}
    }
}

fn try_fold_memoized_member_apply_call(stmts: &[Stmt]) -> Option<(Stmt, usize)> {
    let first = stmts.first()?;
    let first_span = first.span();
    let (method_key, member) = extract_memoized_member_assign(first)?;
    let call = extract_bound_memoized_member_call(stmts.get(1)?, &method_key, &member)?;
    let mut keys = vec![method_key];
    keys.extend(memoized_member_temp_keys(&member));
    if keys
        .iter()
        .any(|key| local_temp_read_before_reassign(&stmts[2..], key))
    {
        return None;
    }

    Some((
        Stmt::Expr(ExprStmt {
            span: first_span,
            expr: Box::new(Expr::Call(call)),
        }),
        2,
    ))
}

struct MemoizedMember {
    receiver: Box<Expr>,
    receiver_key: Option<BindingKey>,
    prop: swc_core::ecma::ast::MemberProp,
    member_span: Span,
}

fn extract_memoized_member_assign(stmt: &Stmt) -> Option<(BindingKey, MemoizedMember)> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let method_ident = assign.left.as_simple()?.as_ident()?;
    if !is_likely_generated_alias(&method_ident.id.sym) {
        return None;
    }
    let Expr::Member(member) = assign.right.as_ref() else {
        return None;
    };
    Some((
        binding_key(&method_ident.id),
        MemoizedMember {
            receiver: memoized_member_receiver(&member.obj)?,
            receiver_key: memoized_receiver_key(&member.obj),
            prop: member.prop.clone(),
            member_span: member.span,
        },
    ))
}

fn memoized_member_receiver(receiver: &Expr) -> Option<Box<Expr>> {
    let receiver = strip_parens(receiver);
    if let Expr::Assign(assign) = receiver {
        if assign.op != AssignOp::Assign {
            return None;
        }
        let left = assign.left.as_simple()?.as_ident()?;
        if !is_likely_generated_alias(&left.id.sym) {
            return None;
        }
        return Some(assign.right.clone());
    }
    Some(Box::new(receiver.clone()))
}

fn memoized_receiver_key(receiver: &Expr) -> Option<BindingKey> {
    let Expr::Assign(assign) = strip_parens(receiver) else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let left = assign.left.as_simple()?.as_ident()?;
    Some(binding_key(&left.id))
}

fn extract_bound_memoized_member_call(
    stmt: &Stmt,
    method_key: &BindingKey,
    member: &MemoizedMember,
) -> Option<CallExpr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr.as_ref() else {
        return None;
    };
    let callee = call.callee.as_expr()?;
    let Expr::Member(call_member) = callee.as_ref() else {
        return None;
    };
    if local_temp_key(&call_member.obj).as_ref() != Some(method_key) {
        return None;
    }

    if is_ident_prop(&call_member.prop, "call") {
        return extract_bound_memoized_call(call, member);
    }
    if is_ident_prop(&call_member.prop, "apply") {
        return extract_bound_memoized_apply(call, member);
    }

    None
}

fn extract_bound_memoized_call(call: &CallExpr, member: &MemoizedMember) -> Option<CallExpr> {
    if call.args.is_empty() || !memoized_call_receiver_matches(&call.args[0].expr, member) {
        return None;
    }
    let mut next = call.clone();
    next.callee = memoized_call_callee(member);
    next.args.remove(0);
    Some(next)
}

fn extract_bound_memoized_apply(call: &CallExpr, member: &MemoizedMember) -> Option<CallExpr> {
    if call.args.len() != 2
        || call.args[0].spread.is_some()
        || call.args[1].spread.is_some()
        || !memoized_call_receiver_matches(&call.args[0].expr, member)
    {
        return None;
    }
    let mut next = call.clone();
    next.callee = memoized_call_callee(member);
    next.args = args_from_apply_arg(call.args[1].expr.clone());
    Some(next)
}

fn memoized_call_callee(member: &MemoizedMember) -> Callee {
    Callee::Expr(Box::new(Expr::Member(MemberExpr {
        span: member.member_span,
        obj: member.receiver.clone(),
        prop: member.prop.clone(),
    })))
}

fn memoized_call_receiver_matches(receiver_arg: &Expr, member: &MemoizedMember) -> bool {
    if let Some(receiver_key) = &member.receiver_key {
        local_temp_key(receiver_arg).as_ref() == Some(receiver_key)
    } else {
        direct_memoized_receiver_matches(receiver_arg, &member.receiver)
    }
}

fn direct_memoized_receiver_matches(receiver_arg: &Expr, receiver: &Expr) -> bool {
    match (strip_parens(receiver_arg), strip_parens(receiver)) {
        (Expr::Ident(left), Expr::Ident(right)) => left.sym == right.sym && left.ctxt == right.ctxt,
        (Expr::This(_), Expr::This(_)) => true,
        _ => false,
    }
}

fn args_from_apply_arg(arg: Box<Expr>) -> Vec<ExprOrSpread> {
    match *arg {
        Expr::Array(array) if array.elems.iter().all(Option::is_some) => {
            array.elems.into_iter().flatten().collect()
        }
        expr => vec![ExprOrSpread {
            spread: Some(DUMMY_SP),
            expr: Box::new(expr),
        }],
    }
}

fn memoized_member_temp_keys(member: &MemoizedMember) -> Vec<BindingKey> {
    let mut keys = Vec::new();
    if let Some(receiver_key) = &member.receiver_key {
        keys.push(receiver_key.clone());
    }
    keys
}

fn local_temp_key(expr: &Expr) -> Option<BindingKey> {
    let Expr::Ident(id) = strip_parens(expr) else {
        return None;
    };
    Some(binding_key(id))
}

fn local_temp_read_before_reassign(stmts: &[Stmt], key: &BindingKey) -> bool {
    for stmt in stmts {
        if let Some(rhs) = direct_local_temp_reassign_rhs(stmt, key) {
            return expr_reads_local_temp(rhs, key);
        }
        if stmt_reads_local_temp(stmt, key) {
            return true;
        }
    }
    false
}

fn direct_local_temp_reassign_rhs<'a>(stmt: &'a Stmt, key: &BindingKey) -> Option<&'a Expr> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign || !assign_target_matches_local_temp(&assign.left, key) {
        return None;
    }
    Some(&assign.right)
}

fn stmt_reads_local_temp(stmt: &Stmt, key: &BindingKey) -> bool {
    let mut finder = LocalTempReadFinder { key, found: false };
    stmt.visit_with(&mut finder);
    finder.found
}

fn expr_reads_local_temp(expr: &Expr, key: &BindingKey) -> bool {
    let mut finder = LocalTempReadFinder { key, found: false };
    expr.visit_with(&mut finder);
    finder.found
}

struct LocalTempReadFinder<'a> {
    key: &'a BindingKey,
    found: bool,
}

impl Visit for LocalTempReadFinder<'_> {
    fn visit_var_declarator(&mut self, decl: &VarDeclarator) {
        if let Some(init) = &decl.init {
            init.visit_with(self);
        }
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if !assign_target_matches_local_temp(&assign.left, self.key) {
            assign.left.visit_with(self);
        }
        assign.right.visit_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.key.0 && ident.ctxt == self.key.1 {
            self.found = true;
        }
    }
}

fn assign_target_matches_local_temp(target: &AssignTarget, key: &BindingKey) -> bool {
    matches!(
        target,
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding))
            if binding.id.sym == key.0 && binding.id.ctxt == key.1
    )
}

// ============================================================
// __awaiter wrapper -> async function
// ============================================================

fn try_transform_awaiter(body: &mut BlockStmt, helpers: &AsyncHelperContext) -> bool {
    // Find: return __awaiter(this, void0, void0, function*() { ... })
    let return_idx = body
        .stmts
        .iter()
        .position(|stmt| is_awaiter_return(stmt, helpers));
    let return_idx = match return_idx {
        Some(i) => i,
        None => return false,
    };

    let inner_stmts = match extract_awaiter_body(body.stmts[return_idx].clone(), helpers) {
        Some(s) => s,
        None => return false,
    };
    body.stmts.remove(return_idx);

    // Replace yield with await in the extracted statements
    let mut inner_stmts = inner_stmts;
    replace_yield_with_await(&mut inner_stmts);

    // Splice the inner statements in place of the return
    body.stmts.splice(return_idx..return_idx, inner_stmts);
    true
}

fn is_awaiter_return(stmt: &Stmt, helpers: &AsyncHelperContext) -> bool {
    let Stmt::Return(ret) = stmt else {
        return false;
    };
    let Some(arg) = &ret.arg else { return false };
    let Expr::Call(call) = arg.as_ref() else {
        return false;
    };
    helpers.is_awaiter_call(call)
}

fn extract_awaiter_body(stmt: Stmt, helpers: &AsyncHelperContext) -> Option<Vec<Stmt>> {
    let Stmt::Return(ret) = stmt else { return None };
    let arg = *ret.arg?;
    let Expr::Call(mut call) = arg else {
        return None;
    };
    if !helpers.is_awaiter_call(&call) {
        return None;
    }
    if call.args.len() < 4 {
        return None;
    }
    let this_arg = classify_awaiter_this_arg(&call.args[0].expr);

    let gen_fn_arg = *call.args.remove(3).expr;
    let Expr::Fn(fn_expr) = gen_fn_arg else {
        return None;
    };
    let body = fn_expr.function.body?;
    let mut stmts = body.stmts;
    apply_awaiter_this_arg(&mut stmts, this_arg)?;
    Some(stmts)
}

/// The state machine's `this` is the awaiter's first argument. Unwrapping the
/// wrapper splices the body into the enclosing function, which rebinds `this`,
/// so anything other than the enclosing `this` must be accounted for.
enum AwaiterThisArg {
    /// `this` (same binding after splicing) or `void 0` / `undefined`
    /// (tsc emits these only where `this` is not used meaningfully).
    Compatible,
    /// A captured alias such as tsc's `var _this = this`; body-level `this`
    /// references are rewritten to it.
    Alias(Ident),
    /// Any other expression would need re-evaluating per `this` reference;
    /// the wrapper is preserved.
    Unsupported,
}

fn classify_awaiter_this_arg(expr: &Expr) -> AwaiterThisArg {
    match strip_parens(expr) {
        Expr::This(_) => AwaiterThisArg::Compatible,
        Expr::Unary(unary) if unary.op == swc_core::ecma::ast::UnaryOp::Void => {
            AwaiterThisArg::Compatible
        }
        Expr::Ident(id) if id.sym.as_ref() == "undefined" => AwaiterThisArg::Compatible,
        Expr::Ident(id) => AwaiterThisArg::Alias(id.clone()),
        _ => AwaiterThisArg::Unsupported,
    }
}

/// Returns `None` when the thisArg cannot be represented after unwrapping.
fn apply_awaiter_this_arg(stmts: &mut [Stmt], this_arg: AwaiterThisArg) -> Option<()> {
    match this_arg {
        AwaiterThisArg::Compatible => Some(()),
        AwaiterThisArg::Unsupported => None,
        AwaiterThisArg::Alias(alias) => {
            if stmts_declare_name(stmts, &alias.sym) {
                return None;
            }
            let mut replacer = ThisToAlias { alias };
            for stmt in stmts.iter_mut() {
                stmt.visit_mut_with(&mut replacer);
            }
            Some(())
        }
    }
}

/// Conservatively detect any binding of `name` anywhere in `stmts` (including
/// scopes the `this` rewrite never reaches); a shadowed alias would be
/// captured by the inner binding once printed.
fn stmts_declare_name(stmts: &[Stmt], name: &Atom) -> bool {
    struct BindingNameFinder<'a> {
        name: &'a Atom,
        found: bool,
    }
    impl Visit for BindingNameFinder<'_> {
        fn visit_pat(&mut self, pat: &Pat) {
            if let Pat::Ident(binding) = pat {
                self.found |= binding.id.sym == *self.name;
            }
            pat.visit_children_with(self);
        }

        fn visit_fn_expr(&mut self, fn_expr: &swc_core::ecma::ast::FnExpr) {
            self.found |= fn_expr
                .ident
                .as_ref()
                .is_some_and(|ident| ident.sym == *self.name);
            fn_expr.visit_children_with(self);
        }

        fn visit_fn_decl(&mut self, fn_decl: &swc_core::ecma::ast::FnDecl) {
            self.found |= fn_decl.ident.sym == *self.name;
            fn_decl.visit_children_with(self);
        }
    }
    let mut finder = BindingNameFinder { name, found: false };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
    }
    finder.found
}

struct ThisToAlias {
    alias: Ident,
}

impl VisitMut for ThisToAlias {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if matches!(expr, Expr::This(_)) {
            *expr = Expr::Ident(self.alias.clone());
            return;
        }
        expr.visit_mut_children_with(self);
    }

    // `this` is lexical only through arrows; anything with its own `this`
    // binding keeps its references untouched.
    fn visit_mut_function(&mut self, _func: &mut Function) {}

    fn visit_mut_class(&mut self, _class: &mut swc_core::ecma::ast::Class) {}
}

/// Transform a standalone `__awaiter(this, void0, void0, function*() {...})`
/// expression into `(async function() {...})()`. Handles the IIFE pattern where
/// `__awaiter(...)` appears at expression level rather than inside a function body.
fn try_transform_awaiter_iife(expr: &mut Expr, helpers: &AsyncHelperContext) {
    let original_span = expr.span();
    let Expr::Call(call) = expr else { return };
    if !helpers.is_awaiter_call(call) || call.args.len() < 4 {
        return;
    }
    let Expr::Fn(fn_expr) = call.args[3].expr.as_ref() else {
        return;
    };
    if !fn_expr.function.params.is_empty() {
        return;
    }
    let Some(body) = &fn_expr.function.body else {
        return;
    };
    if contains_unresolved_generator_wrapper(body, helpers) {
        return;
    }
    let this_arg = classify_awaiter_this_arg(&call.args[0].expr);
    let fn_span = fn_expr.function.span;

    let mut stmts = body.stmts.clone();
    if apply_awaiter_this_arg(&mut stmts, this_arg).is_none() {
        return;
    }
    replace_yield_with_await(&mut stmts);

    let async_fn = Expr::Fn(swc_core::ecma::ast::FnExpr {
        ident: None,
        function: Box::new(Function {
            params: vec![],
            decorators: vec![],
            span: fn_span,
            ctxt: Default::default(),
            body: Some(BlockStmt {
                span: DUMMY_SP,
                ctxt: Default::default(),
                stmts,
            }),
            is_generator: false,
            is_async: true,
            type_params: None,
            return_type: None,
        }),
    });
    *expr = Expr::Call(swc_core::ecma::ast::CallExpr {
        span: original_span,
        ctxt: Default::default(),
        callee: swc_core::ecma::ast::Callee::Expr(Box::new(Expr::Paren(
            swc_core::ecma::ast::ParenExpr {
                span: original_span,
                expr: Box::new(async_fn),
            },
        ))),
        args: vec![],
        type_args: None,
    });
}

fn replace_yield_with_await(stmts: &mut Vec<Stmt>) {
    struct YieldToAwait;
    impl VisitMut for YieldToAwait {
        fn visit_mut_function(&mut self, _func: &mut Function) {}

        fn visit_mut_arrow_expr(&mut self, _arrow: &mut ArrowExpr) {}

        fn visit_mut_expr(&mut self, expr: &mut Expr) {
            if let Expr::Yield(y) = expr {
                let yield_span = y.span;
                let arg = y.arg.take().unwrap_or_else(|| {
                    Box::new(Expr::Ident(Ident::new_no_ctxt(
                        "undefined".into(),
                        DUMMY_SP,
                    )))
                });
                *expr = Expr::Await(AwaitExpr {
                    span: yield_span,
                    arg,
                });
                expr.visit_mut_children_with(self);
                return;
            }
            expr.visit_mut_children_with(self);
        }
    }
    let mut v = YieldToAwait;
    for s in stmts.iter_mut() {
        s.visit_mut_with(&mut v);
    }
}

fn remove_unused_inline_async_helpers(
    module: &mut swc_core::ecma::ast::Module,
    local_helpers: &LocalHelperContext,
) {
    local_helpers.remove_unused_inline_ts_helpers(
        module,
        &[
            TsHelperKind::Awaiter,
            TsHelperKind::Generator,
            TsHelperKind::Values,
        ],
    );
}

fn collect_awaiter_param_hints(
    func: &Function,
    helpers: &AsyncHelperContext,
) -> HashMap<BindingId, Atom> {
    let Some(body) = &func.body else {
        return HashMap::new();
    };
    if !body
        .stmts
        .iter()
        .any(|stmt| is_awaiter_return(stmt, helpers))
    {
        return HashMap::new();
    }

    let param_ids: HashSet<BindingId> = func
        .params
        .iter()
        .filter_map(|param| match &param.pat {
            Pat::Ident(binding) if is_likely_generated_alias(&binding.id.sym) => {
                Some((binding.id.sym.clone(), binding.id.ctxt))
            }
            _ => None,
        })
        .collect();
    if param_ids.is_empty() {
        return HashMap::new();
    }

    #[derive(Default)]
    struct Collector {
        param_ids: HashSet<BindingId>,
        targets: HashMap<BindingId, HashSet<Atom>>,
    }

    impl Visit for Collector {
        fn visit_prop(&mut self, prop: &Prop) {
            if let Prop::KeyValue(kv) = prop {
                if let Expr::Ident(value) = kv.value.as_ref() {
                    let bid = (value.sym.clone(), value.ctxt);
                    if self.param_ids.contains(&bid) {
                        if let Some(target) = key_as_param_hint(&kv.key) {
                            self.targets.entry(bid).or_default().insert(target);
                        }
                    }
                }
            }
            prop.visit_children_with(self);
        }
    }

    let mut collector = Collector {
        param_ids,
        targets: HashMap::new(),
    };
    body.visit_with(&mut collector);

    collector
        .targets
        .into_iter()
        .filter_map(|(bid, targets)| {
            if targets.len() == 1 {
                Some((bid, targets.into_iter().next().unwrap()))
            } else {
                None
            }
        })
        .collect()
}

fn apply_unused_param_hints(func: &mut Function, hints: HashMap<BindingId, Atom>) {
    if hints.is_empty() {
        return;
    }
    let Some(body) = &func.body else { return };

    struct UseCollector {
        uses: HashSet<BindingId>,
    }

    impl Visit for UseCollector {
        fn visit_ident(&mut self, ident: &Ident) {
            self.uses.insert((ident.sym.clone(), ident.ctxt));
        }

        fn visit_pat(&mut self, pat: &Pat) {
            match pat {
                Pat::Array(array) => {
                    for elem in array.elems.iter().flatten() {
                        self.visit_pat(elem);
                    }
                }
                Pat::Object(object) => {
                    for prop in &object.props {
                        match prop {
                            swc_core::ecma::ast::ObjectPatProp::KeyValue(kv) => {
                                if let PropName::Computed(computed) = &kv.key {
                                    computed.expr.visit_with(self);
                                }
                                self.visit_pat(&kv.value);
                            }
                            swc_core::ecma::ast::ObjectPatProp::Assign(assign) => {
                                if let Some(default) = &assign.value {
                                    default.visit_with(self);
                                }
                            }
                            swc_core::ecma::ast::ObjectPatProp::Rest(rest) => {
                                self.visit_pat(&rest.arg);
                            }
                        }
                    }
                }
                Pat::Assign(assign) => {
                    self.visit_pat(&assign.left);
                    assign.right.visit_with(self);
                }
                Pat::Rest(rest) => self.visit_pat(&rest.arg),
                Pat::Expr(expr) => expr.visit_with(self),
                Pat::Ident(_) | Pat::Invalid(_) => {}
            }
        }

        fn visit_prop_name(&mut self, name: &PropName) {
            if let PropName::Computed(computed) = name {
                computed.expr.visit_with(self);
            }
        }
    }

    let mut collector = UseCollector {
        uses: HashSet::new(),
    };
    for param in &func.params {
        collector.visit_pat(&param.pat);
    }
    body.visit_with(&mut collector);

    let mut reserved_param_names: HashSet<Atom> = func
        .params
        .iter()
        .filter_map(|param| match &param.pat {
            Pat::Ident(binding) => Some(binding.id.sym.clone()),
            _ => None,
        })
        .collect();

    for param in &mut func.params {
        let Pat::Ident(binding) = &mut param.pat else {
            continue;
        };
        let bid = (binding.id.sym.clone(), binding.id.ctxt);
        if collector.uses.contains(&bid) {
            continue;
        }
        let Some(target) = hints.get(&bid) else {
            continue;
        };
        if !is_valid_param_hint(target.as_ref()) || reserved_param_names.contains(target) {
            continue;
        }
        reserved_param_names.remove(&binding.id.sym);
        binding.id.sym = target.clone();
        reserved_param_names.insert(target.clone());
    }
}

fn key_as_param_hint(key: &PropName) -> Option<Atom> {
    let raw = match key {
        PropName::Ident(ident) => ident.sym.as_ref(),
        PropName::Str(str_) => str_.value.as_str()?,
        _ => return None,
    };
    if is_valid_param_hint(raw) {
        Some(raw.into())
    } else {
        None
    }
}

fn is_valid_param_hint(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()) {
        return false;
    }
    !matches!(
        value,
        "arguments"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "eval"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

// ============================================================
// Helpers
// ============================================================

fn is_ident_prop(prop: &swc_core::ecma::ast::MemberProp, name: &str) -> bool {
    matches!(prop, swc_core::ecma::ast::MemberProp::Ident(n) if n.sym.as_str() == name)
}
