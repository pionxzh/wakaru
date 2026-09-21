//! `for await` recovery for `UnForOf`.
//!
//! Every lowerer replaces `for await (const item of iterable)` with an async
//! iterator adapter call wrapped in the same three-part protocol:
//!
//! ```js
//! let abrupt = false, didError = false, iteratorError;   // Babel/SWC flags
//! try {
//!   var it = _asyncIterator(iterable);                     // adapter call
//!   for (let step; abrupt = !(step = await it.next()).done; abrupt = false) {
//!     const item = step.value;
//!     // body
//!   }
//! } catch (err) { didError = true; iteratorError = err; }
//! finally {
//!   try { if (abrupt && it.return != null) await it.return(); }
//!   finally { if (didError) throw iteratorError; }
//! }
//! ```
//!
//! The adapter callee identifies the producer (Babel `_asyncIterator`, SWC
//! `_async_iterator`, esbuild `__forAwait`, TypeScript `__asyncValues`); the
//! loop test, catch, and close/rethrow guards are parsed generically because
//! Terser reshapes them independently of the producer. The rewrite runs from
//! `UnForOf::process_stmt_vec`, after `UnAsyncAwait` has restored `await` and
//! `UnVariableMerging` has hoisted the loop-head declarators into statements.
//!
//! Recovering the loop drops the protocol's `return()` guard and rethrow
//! bookkeeping: native `for await` closes the iterator on abrupt exit and
//! propagates the body's error the same way. Babel 7.8–7.13 additionally
//! await `step.value` in the loop head; native `for await` awaits only the
//! result object (`async_iterator_value_await` in rewrite-assumptions).

use crate::collections::HashSet;

use swc_core::atoms::Atom;
use swc_core::common::{Spanned, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, BinExpr, BinaryOp, BlockStmt, CallExpr, Callee, Decl, Expr,
    ExprOrSpread, ExprStmt, ForHead, ForOfStmt, ForStmt, Ident, IdentName, Lit, MemberExpr,
    MemberProp, Module, ModuleItem, NewExpr, Pat, Prop, PropName, PropOrSpread, SimpleAssignTarget,
    Stmt, TryStmt, UnaryOp, VarDeclKind, VarDeclOrExpr,
};
use swc_core::ecma::utils::find_pat_ids;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::utils::paren::strip_parens;

use super::helper_matcher::{binding_key, remove_unused_helper_declarations, BindingKey};
use super::transpiler_helper_utils::{LocalHelperContext, TranspilerHelperKind};
use super::un_for_of::{
    build_helper_for_of, extract_done_obj, is_ident_key, is_iterator_next_call, pat_as_ident,
    stmt_as_try, ForOfHelperContext,
};

/// Fold every recognized async iterator protocol in `stmts` into a
/// `for await` statement, removing the flag declarations it consumed.
pub(super) fn rewrite_async_iterator_loops(stmts: &mut Vec<Stmt>, ctx: &ForOfHelperContext) {
    let mut i = 0;
    while i < stmts.len() {
        if matches!(stmts[i], Stmt::Try(_)) {
            if let Some(rewrite) = try_convert_async_iterator_try(stmts, i, ctx) {
                stmts[i] = Stmt::ForOf(rewrite.for_of);
                let mut removed_before = 0;
                for idx in rewrite.consumed.into_iter().rev() {
                    stmts.remove(idx);
                    if idx < i {
                        removed_before += 1;
                    }
                }
                i -= removed_before;
                ctx.rewrote_async_iterator_loop.set(true);
            }
        }
        i += 1;
    }
}

struct AsyncRewrite {
    for_of: ForOfStmt,
    /// Indices in the original statement list (ascending, excluding the try
    /// statement itself) whose flag declarations the protocol consumed.
    consumed: Vec<usize>,
}

/// The loop head of the protocol, after `await` has been restored.
struct LoopShape {
    iterable: Box<Expr>,
    iterator: Ident,
    step: Ident,
    /// Babel/esbuild: true while the iterator is not done, reset to `false`
    /// at the end of every iteration (`abrupt = !(step = await it.next()).done`).
    abrupt_flag: Option<Ident>,
    /// Babel 7.8–7.13: `normal = step.done`, reset to `true` per iteration.
    normal_flag: Option<Ident>,
    /// TypeScript 5: `first = true` before the loop, `false` once a value
    /// has been read, `true` again at the end of the iteration.
    first_flag: Option<Ident>,
    /// TypeScript 5: `done = step.done` read by the close guard.
    done_temp: Option<Ident>,
    /// The value is read into a temporary before the element binding
    /// (`value = await step.value` in old Babel, `value = step.value` in TS 5).
    value_temp: Option<Ident>,
    /// Flags initialized in the try block before the loop (`first = true`).
    preamble_flags: Vec<Ident>,
    /// Uninitialized temporaries declared in the try block or loop head.
    temps: Vec<Ident>,
    body: BlockStmt,
}

struct CatchShape {
    /// Bindings written by the catch clause (`didError`, `iteratorError`,
    /// esbuild's `error = [err]`, TypeScript's `e_1 = { error: err }`).
    error_idents: Vec<Ident>,
}

fn try_convert_async_iterator_try(
    stmts: &[Stmt],
    index: usize,
    ctx: &ForOfHelperContext,
) -> Option<AsyncRewrite> {
    let try_stmt = stmt_as_try(&stmts[index])?;
    let mut shape = parse_try_block(&try_stmt.block, ctx)?;
    let catch = parse_catch(try_stmt)?;
    let method_temps = parse_finalizer(try_stmt, &shape, &catch)?;

    let mut protocol_idents: Vec<Ident> = vec![shape.iterator.clone(), shape.step.clone()];
    protocol_idents.extend(shape.abrupt_flag.iter().cloned());
    protocol_idents.extend(shape.normal_flag.iter().cloned());
    protocol_idents.extend(shape.first_flag.iter().cloned());
    protocol_idents.extend(shape.done_temp.iter().cloned());
    protocol_idents.extend(shape.value_temp.iter().cloned());
    protocol_idents.extend(shape.preamble_flags.iter().cloned());
    protocol_idents.extend(shape.temps.iter().cloned());
    protocol_idents.extend(catch.error_idents.iter().cloned());
    protocol_idents.extend(method_temps);

    // Strip the protocol's own bookkeeping from the start of the loop body:
    // TypeScript 5 copies the value into a temporary and clears `first`.
    strip_body_protocol_prefix(&mut shape, &mut protocol_idents)?;

    // Flag declarations and initial assignments before the try statement.
    let consumed: Vec<usize> = stmts[..index]
        .iter()
        .enumerate()
        .filter(|(_, stmt)| is_consumable_flag_stmt(stmt, &protocol_idents))
        .map(|(idx, _)| idx)
        .collect();

    // Every protocol temporary must be private to the protocol: its module
    // uses must all sit inside the try statement or the consumed flag
    // statements.
    let mut consumed_stmts: Vec<Stmt> = consumed.iter().map(|idx| stmts[*idx].clone()).collect();
    consumed_stmts.push(stmts[index].clone());
    let local_uses = BindingUseIndex::collect_stmts(&consumed_stmts);
    if protocol_idents.iter().any(|ident| {
        let key = binding_key(ident);
        local_uses.use_count(&key) != ctx.binding_uses.use_count(&key)
    }) {
        return None;
    }

    let mut body = shape.body;
    if let Some(value_temp) = &shape.value_temp {
        redirect_value_temp_to_step(&mut body, value_temp, &shape.step)?;
    }

    let mut for_of = build_helper_for_of(
        body,
        shape.iterable,
        shape.step,
        stmts[index].span(),
        ctx,
        true,
    )?;
    unwrap_single_block_body(&mut for_of);
    Some(AsyncRewrite { for_of, consumed })
}

// ---------------------------------------------------------------------------
// try block: adapter call, temporaries, loop head
// ---------------------------------------------------------------------------

fn parse_try_block(block: &BlockStmt, ctx: &ForOfHelperContext) -> Option<LoopShape> {
    let (for_stmt, preamble) = block.stmts.split_last()?;
    let Stmt::For(for_stmt) = for_stmt else {
        return None;
    };

    let mut iterator_init: Option<(Ident, Box<Expr>)> = None;
    let mut preamble_flags = Vec::new();
    let mut temps = Vec::new();

    for stmt in preamble {
        match stmt {
            Stmt::Decl(Decl::Var(var)) => {
                for decl in &var.decls {
                    let ident = pat_as_ident(&decl.name)?.id.clone();
                    match decl.init.as_deref() {
                        None => temps.push(ident),
                        Some(Expr::Lit(Lit::Bool(_))) => preamble_flags.push(ident),
                        Some(init) => {
                            let iterable = adapter_call_iterable(init, ctx)?;
                            if iterator_init.replace((ident, iterable)).is_some() {
                                return None;
                            }
                        }
                    }
                }
            }
            Stmt::Expr(ExprStmt { expr, .. }) => {
                let Expr::Assign(assign) = strip_parens(expr) else {
                    return None;
                };
                let ident = assign_target_ident(assign)?;
                match strip_parens(&assign.right) {
                    Expr::Lit(Lit::Bool(_)) => preamble_flags.push(ident),
                    init => {
                        let iterable = adapter_call_iterable(init, ctx)?;
                        if iterator_init.replace((ident, iterable)).is_some() {
                            return None;
                        }
                    }
                }
            }
            _ => return None,
        }
    }

    // Loop head declarators that UnVariableMerging left in place.
    match &for_stmt.init {
        None => {}
        Some(VarDeclOrExpr::VarDecl(init_decl)) => {
            for decl in &init_decl.decls {
                let ident = pat_as_ident(&decl.name)?.id.clone();
                match decl.init.as_deref() {
                    None => temps.push(ident),
                    Some(Expr::Lit(Lit::Bool(_))) => preamble_flags.push(ident),
                    Some(init) => {
                        let iterable = adapter_call_iterable(init, ctx)?;
                        if iterator_init.replace((ident, iterable)).is_some() {
                            return None;
                        }
                    }
                }
            }
        }
        Some(VarDeclOrExpr::Expr(_)) => return None,
    }

    let (iterator, iterable) = iterator_init?;
    let mut shape = parse_loop_test(for_stmt.test.as_deref()?, &iterator)?;
    parse_loop_update(for_stmt, &mut shape, &preamble_flags)?;

    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    Some(LoopShape {
        iterable,
        iterator,
        step: shape.step,
        abrupt_flag: shape.abrupt_flag,
        normal_flag: shape.normal_flag,
        first_flag: shape.first_flag,
        done_temp: shape.done_temp,
        value_temp: shape.value_temp,
        preamble_flags,
        temps,
        body: body.clone(),
    })
}

/// `adapter(iterable)` where the adapter is a recognized async iterator
/// helper: Babel/SWC `_asyncIterator`, esbuild `__forAwait`, or tslib
/// `__asyncValues`.
fn adapter_call_iterable(expr: &Expr, ctx: &ForOfHelperContext) -> Option<Box<Expr>> {
    let Expr::Call(CallExpr { callee, args, .. }) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = callee else {
        return None;
    };
    let recognized = ctx
        .local_helpers
        .is_helper_callee(strip_parens(callee), TranspilerHelperKind::AsyncIterator)
        || ctx.is_ts_async_values_callee_expr(callee)
        || matches!(strip_parens(callee), Expr::Ident(id)
            if ctx.esbuild_for_await_helpers.contains(&binding_key(id)));
    if !recognized {
        return None;
    }
    let [ExprOrSpread { spread: None, expr }] = args.as_slice() else {
        return None;
    };
    Some(expr.clone())
}

struct LoopTest {
    step: Ident,
    abrupt_flag: Option<Ident>,
    normal_flag: Option<Ident>,
    first_flag: Option<Ident>,
    done_temp: Option<Ident>,
    value_temp: Option<Ident>,
}

/// The loop test after `await` is restored. Four producer shapes:
///
/// - Babel ≥ 7.28, SWC, esbuild: `abrupt = !(step = await it.next()).done`
/// - Babel 7.8–7.13: `step = await it.next(), normal = step.done,
///   value = await step.value, !normal`
/// - TypeScript 5: `step = await it.next(), done = step.done, !done`
/// - TypeScript 4: `step = await it.next(), !step.done`
fn parse_loop_test(test: &Expr, iterator: &Ident) -> Option<LoopTest> {
    match strip_parens(test) {
        Expr::Assign(assign) => {
            let abrupt_flag = assign_target_ident(assign)?;
            let Expr::Unary(unary) = strip_parens(&assign.right) else {
                return None;
            };
            if unary.op != UnaryOp::Bang {
                return None;
            }
            let Expr::Assign(step_assign) = extract_done_obj(&unary.arg)? else {
                return None;
            };
            let step = assign_target_ident(step_assign)?;
            if !is_awaited_next_call(&step_assign.right, iterator) {
                return None;
            }
            Some(LoopTest {
                step,
                abrupt_flag: Some(abrupt_flag),
                normal_flag: None,
                first_flag: None,
                done_temp: None,
                value_temp: None,
            })
        }
        Expr::Seq(seq) => {
            let (first, rest) = seq.exprs.split_first()?;
            let Expr::Assign(first_assign) = strip_parens(first) else {
                return None;
            };
            let first_target = assign_target_ident(first_assign)?;
            // Terser folds `step = await it.next(), normal = step.done` into
            // `normal = (step = await it.next()).done`.
            let (step, merged_done_flag) = if is_awaited_next_call(&first_assign.right, iterator) {
                (first_target, None)
            } else {
                let Expr::Assign(step_assign) =
                    extract_done_obj(strip_parens(&first_assign.right))?
                else {
                    return None;
                };
                let step = assign_target_ident(step_assign)?;
                if !is_awaited_next_call(&step_assign.right, iterator) {
                    return None;
                }
                (step, Some(first_target))
            };
            let (last, middle) = rest.split_last()?;
            let Expr::Unary(unary) = strip_parens(last) else {
                return None;
            };
            if unary.op != UnaryOp::Bang {
                return None;
            }
            match (merged_done_flag, middle) {
                // `!step.done`
                (None, []) => {
                    if !is_done_member_of(&unary.arg, &step) {
                        return None;
                    }
                    Some(LoopTest {
                        step,
                        abrupt_flag: None,
                        normal_flag: None,
                        first_flag: None,
                        done_temp: None,
                        value_temp: None,
                    })
                }
                // `normal = (step = await it.next()).done, value = await step.value, !normal`
                (Some(normal_flag), [value_assign]) => {
                    if !is_ident_key(strip_parens(&unary.arg), &normal_flag) {
                        return None;
                    }
                    let Expr::Assign(value_assign) = strip_parens(value_assign) else {
                        return None;
                    };
                    let value_temp = assign_target_ident(value_assign)?;
                    let Expr::Await(awaited) = strip_parens(&value_assign.right) else {
                        return None;
                    };
                    if !is_value_member_of(&awaited.arg, &step) {
                        return None;
                    }
                    Some(LoopTest {
                        step,
                        abrupt_flag: None,
                        normal_flag: Some(normal_flag),
                        first_flag: None,
                        done_temp: None,
                        value_temp: Some(value_temp),
                    })
                }
                (Some(_), _) => None,
                // `done = step.done, !done`
                (None, [done_assign]) => {
                    let Expr::Assign(done_assign) = strip_parens(done_assign) else {
                        return None;
                    };
                    let done_temp = assign_target_ident(done_assign)?;
                    if !is_done_member_of(&done_assign.right, &step)
                        || !is_ident_key(strip_parens(&unary.arg), &done_temp)
                    {
                        return None;
                    }
                    Some(LoopTest {
                        step,
                        abrupt_flag: None,
                        normal_flag: None,
                        first_flag: None,
                        done_temp: Some(done_temp),
                        value_temp: None,
                    })
                }
                // `normal = step.done, value = await step.value, !normal`
                (None, [normal_assign, value_assign]) => {
                    let Expr::Assign(normal_assign) = strip_parens(normal_assign) else {
                        return None;
                    };
                    let normal_flag = assign_target_ident(normal_assign)?;
                    if !is_done_member_of(&normal_assign.right, &step)
                        || !is_ident_key(strip_parens(&unary.arg), &normal_flag)
                    {
                        return None;
                    }
                    let Expr::Assign(value_assign) = strip_parens(value_assign) else {
                        return None;
                    };
                    let value_temp = assign_target_ident(value_assign)?;
                    let Expr::Await(awaited) = strip_parens(&value_assign.right) else {
                        return None;
                    };
                    if !is_value_member_of(&awaited.arg, &step) {
                        return None;
                    }
                    Some(LoopTest {
                        step,
                        abrupt_flag: None,
                        normal_flag: Some(normal_flag),
                        first_flag: None,
                        done_temp: None,
                        value_temp: Some(value_temp),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The loop update must reset the protocol flag the test sets (or be absent
/// for the TypeScript 4 shape). TypeScript 5 resets its `first` flag here.
fn parse_loop_update(
    for_stmt: &ForStmt,
    shape: &mut LoopTest,
    preamble_flags: &[Ident],
) -> Option<()> {
    let Some(update) = for_stmt.update.as_deref() else {
        return (shape.abrupt_flag.is_none() && shape.normal_flag.is_none()).then_some(());
    };
    let Expr::Assign(assign) = strip_parens(update) else {
        return None;
    };
    let target = assign_target_ident(assign)?;
    let Expr::Lit(Lit::Bool(value)) = strip_parens(&assign.right) else {
        return None;
    };
    if let Some(abrupt) = &shape.abrupt_flag {
        return (same_binding(abrupt, &target) && !value.value).then_some(());
    }
    if let Some(normal) = &shape.normal_flag {
        return (same_binding(normal, &target) && value.value).then_some(());
    }
    // TypeScript 5: `first = true` in the try block, `first = true` per iteration.
    if value.value
        && preamble_flags
            .iter()
            .any(|flag| same_binding(flag, &target))
    {
        shape.first_flag = Some(target);
        return Some(());
    }
    None
}

/// TypeScript 5 starts the body with `value = step.value; first = false;`.
/// Consume those so the element extraction sees the declaration first.
fn strip_body_protocol_prefix(
    shape: &mut LoopShape,
    protocol_idents: &mut Vec<Ident>,
) -> Option<()> {
    let mut consumed = 0;
    for stmt in shape.body.stmts.iter().take(2) {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            break;
        };
        let Expr::Assign(assign) = strip_parens(expr) else {
            break;
        };
        let Some(target) = assign_target_ident(assign) else {
            break;
        };
        if is_value_member_of(&assign.right, &shape.step) {
            if shape.value_temp.is_some() {
                return None;
            }
            protocol_idents.push(target.clone());
            shape.value_temp = Some(target);
            consumed += 1;
            continue;
        }
        if let Some(first) = &shape.first_flag {
            if same_binding(first, &target)
                && matches!(strip_parens(&assign.right), Expr::Lit(Lit::Bool(b)) if !b.value)
            {
                consumed += 1;
                continue;
            }
        }
        break;
    }
    // A `first` flag that the body never clears is not the TS 5 protocol.
    if shape.first_flag.is_some() && consumed < 2 {
        return None;
    }
    shape.body.stmts.drain(..consumed);
    Some(())
}

/// Point the element at `step.value` so the shared builder can recover the
/// pattern: rewrite the element declaration's initializer when the body
/// declares one, otherwise (Terser inlined the element) rewrite every read of
/// the value temporary. The temporary must not be written in the body.
fn redirect_value_temp_to_step(
    body: &mut BlockStmt,
    value_temp: &Ident,
    step: &Ident,
) -> Option<()> {
    if BindingUseIndex::collect_stmts(&body.stmts).has_direct_write(&binding_key(value_temp)) {
        return None;
    }
    let declared = match body.stmts.first_mut() {
        Some(Stmt::Decl(Decl::Var(var))) => match var.decls.as_mut_slice() {
            [decl] => match decl.init.as_deref_mut() {
                Some(init) if is_ident_key(strip_parens(init), value_temp) => {
                    *init = step_value_member(step);
                    true
                }
                _ => false,
            },
            _ => false,
        },
        _ => false,
    };
    if declared {
        return (!body
            .stmts
            .iter()
            .any(|stmt| stmt_mentions_binding(stmt, value_temp)))
        .then_some(());
    }
    let mut replacer = ValueTempReplacer {
        key: binding_key(value_temp),
        replacement: step_value_member(step),
    };
    for stmt in body.stmts.iter_mut() {
        stmt.visit_mut_with(&mut replacer);
    }
    Some(())
}

struct ValueTempReplacer {
    key: BindingKey,
    replacement: Expr,
}

impl VisitMut for ValueTempReplacer {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Ident(id) = expr {
            if id.sym == self.key.0 && id.ctxt == self.key.1 {
                *expr = self.replacement.clone();
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}

// ---------------------------------------------------------------------------
// catch / finally protocol
// ---------------------------------------------------------------------------

/// `catch (err) { didError = true; iteratorError = err; }` (Babel/SWC),
/// `catch (err) { error = [err]; }` (esbuild), or
/// `catch (err) { e_1 = { error: err }; }` (TypeScript).
fn parse_catch(try_stmt: &TryStmt) -> Option<CatchShape> {
    let handler = try_stmt.handler.as_ref()?;
    let param = pat_as_ident(handler.param.as_ref()?)?.id.clone();
    if handler.body.stmts.is_empty() || handler.body.stmts.len() > 2 {
        return None;
    }
    let mut error_idents = Vec::new();
    let mut param_captured = false;
    for stmt in &handler.body.stmts {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return None;
        };
        let Expr::Assign(assign) = strip_parens(expr) else {
            return None;
        };
        let target = assign_target_ident(assign)?;
        match strip_parens(&assign.right) {
            Expr::Lit(Lit::Bool(flag)) if flag.value => {}
            Expr::Ident(id) if is_ident_key(&Expr::Ident(id.clone()), &param) => {
                param_captured = true;
            }
            Expr::Array(array) => {
                let [Some(ExprOrSpread { spread: None, expr })] = array.elems.as_slice() else {
                    return None;
                };
                if !is_ident_key(strip_parens(expr), &param) {
                    return None;
                }
                param_captured = true;
            }
            Expr::Object(object) => {
                let [PropOrSpread::Prop(prop)] = object.props.as_slice() else {
                    return None;
                };
                let Prop::KeyValue(kv) = prop.as_ref() else {
                    return None;
                };
                if !matches!(&kv.key, PropName::Ident(key) if key.sym.as_ref() == "error")
                    || !is_ident_key(strip_parens(&kv.value), &param)
                {
                    return None;
                }
                param_captured = true;
            }
            _ => return None,
        }
        error_idents.push(target);
    }
    param_captured.then_some(CatchShape { error_idents })
}

/// `finally { try { CLOSE } finally { RETHROW } }`. Returns the method
/// temporaries the close guard assigns (`(ret = it.return)`).
fn parse_finalizer(
    try_stmt: &TryStmt,
    shape: &LoopShape,
    catch: &CatchShape,
) -> Option<Vec<Ident>> {
    let finalizer = try_stmt.finalizer.as_ref()?;
    let [Stmt::Try(inner)] = finalizer.stmts.as_slice() else {
        return None;
    };
    if inner.handler.is_some() {
        return None;
    }
    let method_temps = parse_close_block(&inner.block, shape)?;
    parse_rethrow_block(inner.finalizer.as_ref()?, catch)?;
    Some(method_temps)
}

/// `if (<guard>) await it.return();` or `if (<guard>) await ret.call(it);`.
/// The guard may only read the protocol's flags, the step, the iterator's
/// `return`/`done` members, and assign the method temporary.
fn parse_close_block(block: &BlockStmt, shape: &LoopShape) -> Option<Vec<Ident>> {
    let [stmt] = block.stmts.as_slice() else {
        return None;
    };
    let (guard, close) = match stmt {
        Stmt::If(if_stmt) => {
            if if_stmt.alt.is_some() {
                return None;
            }
            (if_stmt.test.as_ref(), single_stmt(&if_stmt.cons)?)
        }
        _ => return None,
    };

    let mut allowed: Vec<Ident> = vec![shape.iterator.clone(), shape.step.clone()];
    allowed.extend(shape.abrupt_flag.iter().cloned());
    allowed.extend(shape.normal_flag.iter().cloned());
    allowed.extend(shape.first_flag.iter().cloned());
    allowed.extend(shape.done_temp.iter().cloned());
    let mut method_temps = Vec::new();
    if !guard_reads_only_protocol(guard, &shape.iterator, &allowed, &mut method_temps) {
        return None;
    }
    // The guard must actually consult iteration state, not just the method.
    let consults_state = shape
        .abrupt_flag
        .iter()
        .chain(shape.normal_flag.iter())
        .chain(shape.first_flag.iter())
        .chain(shape.done_temp.iter())
        .chain(std::iter::once(&shape.step))
        .any(|ident| expr_mentions_binding(guard, ident));
    if !consults_state {
        return None;
    }

    let Stmt::Expr(ExprStmt { expr, .. }) = close else {
        return None;
    };
    let Expr::Await(awaited) = strip_parens(expr) else {
        return None;
    };
    let Expr::Call(call) = strip_parens(&awaited.arg) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(member) = strip_parens(callee) else {
        return None;
    };
    let ok = match member_prop(&member.prop) {
        // `it.return()`
        Some("return") => call.args.is_empty() && is_ident_key(&member.obj, &shape.iterator),
        // `ret.call(it)` where `ret = it.return` was assigned in the guard
        Some("call") => {
            matches!(strip_parens(&member.obj), Expr::Ident(id)
                if method_temps.iter().any(|temp| same_binding(temp, id)))
                && matches!(call.args.as_slice(), [ExprOrSpread { spread: None, expr }]
                    if is_ident_key(strip_parens(expr), &shape.iterator))
        }
        _ => false,
    };
    ok.then_some(method_temps)
}

fn guard_reads_only_protocol(
    expr: &Expr,
    iterator: &Ident,
    allowed: &[Ident],
    method_temps: &mut Vec<Ident>,
) -> bool {
    match strip_parens(expr) {
        Expr::Ident(id) => allowed.iter().any(|ident| same_binding(ident, id)),
        Expr::Lit(Lit::Null(_)) | Expr::Lit(Lit::Bool(_)) => true,
        Expr::Member(member) => {
            matches!(member_prop(&member.prop), Some("return") | Some("done"))
                && matches!(strip_parens(&member.obj), Expr::Ident(id)
                    if allowed.iter().any(|ident| same_binding(ident, id)))
        }
        Expr::Unary(unary) => {
            unary.op == UnaryOp::Bang
                && guard_reads_only_protocol(&unary.arg, iterator, allowed, method_temps)
        }
        Expr::Bin(BinExpr {
            op, left, right, ..
        }) => {
            matches!(
                op,
                BinaryOp::LogicalAnd
                    | BinaryOp::LogicalOr
                    | BinaryOp::NotEq
                    | BinaryOp::NotEqEq
                    | BinaryOp::EqEq
                    | BinaryOp::EqEqEq
            ) && guard_reads_only_protocol(left, iterator, allowed, method_temps)
                && guard_reads_only_protocol(right, iterator, allowed, method_temps)
        }
        // `(ret = it.return)`
        Expr::Assign(assign) => {
            let Some(target) = assign_target_ident(assign) else {
                return false;
            };
            let Expr::Member(member) = strip_parens(&assign.right) else {
                return false;
            };
            if member_prop(&member.prop) != Some("return")
                || !is_ident_key(strip_parens(&member.obj), iterator)
            {
                return false;
            }
            method_temps.push(target);
            true
        }
        _ => false,
    }
}

/// `if (didError) throw iteratorError;` / `if (error) throw error[0];` /
/// `if (e_1) throw e_1.error;`.
fn parse_rethrow_block(block: &BlockStmt, catch: &CatchShape) -> Option<()> {
    let [Stmt::If(if_stmt)] = block.stmts.as_slice() else {
        return None;
    };
    if if_stmt.alt.is_some() {
        return None;
    }
    let Expr::Ident(test) = strip_parens(&if_stmt.test) else {
        return None;
    };
    if !catch
        .error_idents
        .iter()
        .any(|ident| same_binding(ident, test))
    {
        return None;
    }
    let Stmt::Throw(throw) = single_stmt(&if_stmt.cons)? else {
        return None;
    };
    let source = match strip_parens(&throw.arg) {
        Expr::Ident(_) => strip_parens(&throw.arg),
        Expr::Member(member) => match &member.prop {
            MemberProp::Ident(prop) if prop.sym.as_ref() == "error" => strip_parens(&member.obj),
            MemberProp::Computed(computed) if matches!(&*computed.expr, Expr::Lit(Lit::Num(n)) if n.value == 0.0) => {
                strip_parens(&member.obj)
            }
            _ => return None,
        },
        _ => return None,
    };
    let Expr::Ident(source) = source else {
        return None;
    };
    catch
        .error_idents
        .iter()
        .any(|ident| same_binding(ident, source))
        .then_some(())
}

// ---------------------------------------------------------------------------
// flag declarations before the try statement
// ---------------------------------------------------------------------------

/// A declaration of protocol temporaries (`let abrupt = false;`, `let err;`)
/// or an initial flag assignment (`abrupt = false;`) that the protocol owns.
fn is_consumable_flag_stmt(stmt: &Stmt, protocol_idents: &[Ident]) -> bool {
    let owns = |id: &Ident| protocol_idents.iter().any(|ident| same_binding(ident, id));
    match stmt {
        Stmt::Decl(Decl::Var(var)) => var.decls.iter().all(|decl| {
            pat_as_ident(&decl.name).is_some_and(|binding| owns(&binding.id))
                && matches!(
                    decl.init.as_deref().map(strip_parens),
                    None | Some(Expr::Lit(Lit::Bool(_)))
                )
        }),
        Stmt::Expr(ExprStmt { expr, .. }) => {
            let Expr::Assign(assign) = strip_parens(expr) else {
                return false;
            };
            assign_target_ident(assign).is_some_and(|target| owns(&target))
                && matches!(strip_parens(&assign.right), Expr::Lit(Lit::Bool(_)))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// body cleanup
// ---------------------------------------------------------------------------

/// Babel nests the original loop body as a block after the element
/// declaration. Once the declaration moved into the loop head, that block is
/// the whole body; unwrap it unless one of its lexical declarations would
/// collide with a loop-head binding.
fn unwrap_single_block_body(for_of: &mut ForOfStmt) {
    let head_names: HashSet<Atom> = match &for_of.left {
        ForHead::VarDecl(decl) => decl
            .decls
            .iter()
            .flat_map(|decl| find_pat_ids::<_, Ident>(&decl.name))
            .map(|ident| ident.sym)
            .collect(),
        _ => return,
    };
    let Stmt::Block(body) = &mut *for_of.body else {
        return;
    };
    let [Stmt::Block(inner)] = body.stmts.as_slice() else {
        return;
    };
    if inner
        .stmts
        .iter()
        .any(|stmt| declares_lexical_name_in(stmt, &head_names))
    {
        return;
    }
    let inner = inner.clone();
    body.stmts = inner.stmts;
}

fn declares_lexical_name_in(stmt: &Stmt, names: &HashSet<Atom>) -> bool {
    match stmt {
        Stmt::Decl(Decl::Var(var)) if var.kind != VarDeclKind::Var => var
            .decls
            .iter()
            .flat_map(|decl| find_pat_ids::<_, Ident>(&decl.name))
            .any(|ident| names.contains(&ident.sym)),
        Stmt::Decl(Decl::Class(class)) => names.contains(&class.ident.sym),
        Stmt::Decl(Decl::Fn(func)) => names.contains(&func.ident.sym),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// esbuild helper detection and cleanup
// ---------------------------------------------------------------------------

/// esbuild's `__forAwait` adapter and the `__knownSymbol` lookup it calls,
/// matched by body shape so the mangled forms are found too.
pub(super) fn collect_esbuild_for_await_helpers(
    module: &Module,
) -> (HashSet<BindingKey>, HashSet<BindingKey>) {
    let mut for_await = HashSet::default();
    let mut known_symbol = HashSet::default();
    for item in &module.body {
        let ModuleItem::Stmt(Stmt::Decl(Decl::Var(var))) = item else {
            continue;
        };
        for decl in &var.decls {
            let Pat::Ident(binding) = &decl.name else {
                continue;
            };
            let Some(init) = decl.init.as_deref() else {
                continue;
            };
            let Some(param_len) = callable_param_len(init) else {
                continue;
            };
            let mut finder = EsbuildHelperFinder::default();
            init.visit_with(&mut finder);
            if param_len == 3 && finder.is_for_await() {
                for_await.insert(binding_key(&binding.id));
            } else if param_len == 2 && finder.is_known_symbol() {
                known_symbol.insert(binding_key(&binding.id));
            }
        }
    }
    (for_await, known_symbol)
}

fn callable_param_len(expr: &Expr) -> Option<usize> {
    match strip_parens(expr) {
        Expr::Arrow(arrow) => Some(arrow.params.len()),
        Expr::Fn(fn_expr) => Some(fn_expr.function.params.len()),
        _ => None,
    }
}

#[derive(Default)]
struct EsbuildHelperFinder {
    async_iterator_str: bool,
    iterator_str: bool,
    new_promise: bool,
    method_call: bool,
    symbol_computed: bool,
    symbol_for: bool,
}

impl EsbuildHelperFinder {
    /// `(obj, it, method) => (it = obj[__knownSymbol("asyncIterator")]) ?
    /// it.call(obj) : (obj = obj[__knownSymbol("iterator")](), …, new Promise(…))`
    fn is_for_await(&self) -> bool {
        self.async_iterator_str && self.iterator_str && self.new_promise && self.method_call
    }

    /// `(name, symbol) => (symbol = Symbol[name]) ? symbol : Symbol.for("Symbol." + name)`
    fn is_known_symbol(&self) -> bool {
        self.symbol_computed && self.symbol_for
    }
}

impl Visit for EsbuildHelperFinder {
    fn visit_lit(&mut self, lit: &Lit) {
        if let Lit::Str(s) = lit {
            match s.value.as_str() {
                Some("asyncIterator") => self.async_iterator_str = true,
                Some("iterator") => self.iterator_str = true,
                _ => {}
            }
        }
    }

    fn visit_new_expr(&mut self, new_expr: &NewExpr) {
        if matches!(new_expr.callee.as_ref(), Expr::Ident(id) if id.sym.as_ref() == "Promise") {
            self.new_promise = true;
        }
        new_expr.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = strip_parens(callee) {
                match member_prop(&member.prop) {
                    Some("call") => self.method_call = true,
                    Some("for") if is_symbol_ident(&member.obj) => self.symbol_for = true,
                    _ => {}
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        if matches!(member.prop, MemberProp::Computed(_)) && is_symbol_ident(&member.obj) {
            self.symbol_computed = true;
        }
        member.visit_children_with(self);
    }
}

fn is_symbol_ident(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Ident(id) if id.sym.as_ref() == "Symbol")
}

/// Remove adapter helpers whose every call site was folded into `for await`.
pub(super) fn remove_consumed_async_iterator_helpers(
    module: &mut Module,
    local_helpers: &LocalHelperContext,
    ctx: &ForOfHelperContext,
) {
    let roots = local_helpers.helpers_of_kind(TranspilerHelperKind::AsyncIterator);
    if !roots.is_empty() {
        local_helpers.remove_helpers_with_dependencies(module, roots);
    }
    let esbuild: HashSet<BindingKey> = ctx
        .esbuild_for_await_helpers
        .iter()
        .chain(ctx.esbuild_known_symbol_helpers.iter())
        .cloned()
        .collect();
    if !esbuild.is_empty() {
        remove_unused_helper_declarations(module, &esbuild);
    }
}

// ---------------------------------------------------------------------------
// small predicates
// ---------------------------------------------------------------------------

fn same_binding(a: &Ident, b: &Ident) -> bool {
    a.sym == b.sym && a.ctxt == b.ctxt
}

fn assign_target_ident(assign: &AssignExpr) -> Option<Ident> {
    if assign.op != AssignOp::Assign {
        return None;
    }
    match &assign.left {
        AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => Some(binding.id.clone()),
        _ => None,
    }
}

fn is_awaited_next_call(expr: &Expr, iterator: &Ident) -> bool {
    let Expr::Await(awaited) = strip_parens(expr) else {
        return false;
    };
    is_iterator_next_call(strip_parens(&awaited.arg), iterator)
}

fn is_done_member_of(expr: &Expr, step: &Ident) -> bool {
    extract_done_obj(strip_parens(expr)).is_some_and(|obj| is_ident_key(obj, step))
}

fn is_value_member_of(expr: &Expr, step: &Ident) -> bool {
    let Expr::Member(member) = strip_parens(expr) else {
        return false;
    };
    member_prop(&member.prop) == Some("value") && is_ident_key(strip_parens(&member.obj), step)
}

fn member_prop(prop: &MemberProp) -> Option<&str> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.as_ref()),
        _ => None,
    }
}

fn step_value_member(step: &Ident) -> Expr {
    Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(step.clone())),
        prop: MemberProp::Ident(IdentName::new("value".into(), DUMMY_SP)),
    })
}

fn single_stmt(stmt: &Stmt) -> Option<&Stmt> {
    match stmt {
        Stmt::Block(block) => match block.stmts.as_slice() {
            [inner] => Some(inner),
            _ => None,
        },
        other => Some(other),
    }
}

fn stmt_mentions_binding(stmt: &Stmt, ident: &Ident) -> bool {
    let mut finder = BindingMentionFinder {
        key: binding_key(ident),
        found: false,
    };
    stmt.visit_with(&mut finder);
    finder.found
}

fn expr_mentions_binding(expr: &Expr, ident: &Ident) -> bool {
    let mut finder = BindingMentionFinder {
        key: binding_key(ident),
        found: false,
    };
    expr.visit_with(&mut finder);
    finder.found
}

struct BindingMentionFinder {
    key: BindingKey,
    found: bool,
}

impl Visit for BindingMentionFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        if ident.sym == self.key.0 && ident.ctxt == self.key.1 {
            self.found = true;
        }
    }
}
