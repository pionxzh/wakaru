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

    let mut body = shape.body;
    if let Some(value_temp) = &shape.value_temp {
        redirect_value_temp_to_step(&mut body, value_temp, &shape.step)?;
    }

    // A state-machine decoder hoists the element to the function scope, so
    // the body starts with `item = step.value` instead of a declaration. The
    // element becomes the loop binding when nothing outside the protocol
    // reads it; otherwise the loop assigns the outer binding in place.
    let HoistedElement {
        element: hoisted_element,
        consumed_temps,
    } = take_hoisted_element(&mut body, &shape.step);
    protocol_idents.extend(consumed_temps);
    // A closure in the body can read the hoisted element after the loop, so a
    // per-iteration binding would change what it sees; assign in place then.
    let element_is_private = hoisted_element.as_ref().is_some_and(|element| {
        !expr_mentions_binding(&shape.iterable, element)
            && uses_are_within(ctx, element, std::slice::from_ref(&stmts[index]))
            && !nested_function_mentions_binding(&body.stmts, element)
    });
    if element_is_private {
        protocol_idents.extend(hoisted_element.iter().cloned());
    }

    // The privacy check below proves nothing outside the try reads a protocol
    // temporary; the body must not read one either, since its declaration goes
    // with the protocol. The step (folded into the element by the builder) and
    // the element itself are the only temporaries the body may mention.
    let body_may_mention = |ident: &Ident| {
        same_binding(ident, &shape.step)
            || hoisted_element
                .as_ref()
                .is_some_and(|element| same_binding(element, ident))
    };
    if protocol_idents.iter().any(|ident| {
        !body_may_mention(ident)
            && body
                .stmts
                .iter()
                .any(|stmt| stmt_mentions_binding(stmt, ident))
    }) {
        return None;
    }

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

    let for_of = match hoisted_element {
        Some(element) => build_hoisted_element_for_await(
            body,
            shape.iterable,
            element,
            element_is_private,
            stmts[index].span(),
        )?,
        None => {
            let mut for_of = build_helper_for_of(
                body,
                shape.iterable,
                shape.step,
                stmts[index].span(),
                ctx,
                true,
            )?;
            unwrap_single_block_body(&mut for_of);
            for_of
        }
    };

    // Temporaries declared in an enclosing statement list (a decoder hoists
    // them to the function scope) are proven private above; their now-dead
    // declarations are removed when that list is processed.
    let declared_here: Vec<Ident> = consumed
        .iter()
        .flat_map(|idx| declared_idents(&stmts[*idx]))
        .collect();
    let mut orphaned = ctx.orphaned_protocol_temps.borrow_mut();
    for ident in &protocol_idents {
        if !declared_here
            .iter()
            .any(|declared| same_binding(declared, ident))
        {
            orphaned.insert(binding_key(ident));
        }
    }
    Some(AsyncRewrite { for_of, consumed })
}

#[derive(Default)]
struct HoistedElement {
    /// The hoisted binding the loop should bind or assign, when the body
    /// starts by assigning `step.value` (possibly through a temporary) to it.
    element: Option<Ident>,
    /// Value temporaries the fold consumed (`_d` in `_d = step.value; item =
    /// _d;`); they join the protocol temporaries so their hoisted
    /// declarations go with the loop.
    consumed_temps: Vec<Ident>,
}

/// Fold the element assignment a decoder leaves after hoisting the element:
///
/// - `item = step.value;` binds or assigns `item`;
/// - `_d = step.value; item = _d;` (TypeScript's temporary) does the same and
///   consumes `_d`;
/// - `_d = step.value; const item = _d;` (or a destructuring declaration)
///   redirects the declaration to `step.value` and leaves it to the shared
///   builder, consuming `_d`;
/// - `_d = step.value; use(item = _d)` (Terser inlined the alias into the
///   first statement) binds `item` and drops the assignment expression when
///   that assignment is the only read of `_d` and the only mention of `item`
///   in that statement;
/// - `_d = step.value;` followed by member reads of `_d` binds `_d` itself.
///
/// `step` must not be read anywhere else in the body.
fn take_hoisted_element(body: &mut BlockStmt, step: &Ident) -> HoistedElement {
    let Some(element) = leading_assignment(body, |right| is_value_member_of(right, step)) else {
        return HoistedElement::default();
    };
    if same_binding(&element, step)
        || body.stmts[1..]
            .iter()
            .any(|stmt| stmt_mentions_binding(stmt, step))
    {
        return HoistedElement::default();
    }
    body.stmts.remove(0);
    let rest_reads_element = |stmts: &[Stmt]| {
        stmts
            .iter()
            .any(|stmt| stmt_mentions_binding(stmt, &element))
    };

    // `item = _d;`
    if let Some(alias) = leading_assignment(
        body,
        |right| matches!(strip_parens(right), Expr::Ident(id) if same_binding(id, &element)),
    ) {
        if !rest_reads_element(&body.stmts[1..]) {
            body.stmts.remove(0);
            return HoistedElement {
                element: Some(alias),
                consumed_temps: vec![element],
            };
        }
    }

    // `const item = _d;` / `const { id } = _d;`
    let rest_is_free_of_element = !rest_reads_element(&body.stmts[1..]);
    let declared_alias = match body.stmts.first_mut() {
        Some(Stmt::Decl(Decl::Var(var))) => match var.decls.as_mut_slice() {
            [decl] => match decl.init.as_deref_mut() {
                Some(init) if matches!(strip_parens(init), Expr::Ident(id) if same_binding(id, &element)) => {
                    Some(init)
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    };
    if let Some(init) = declared_alias {
        if rest_is_free_of_element {
            *init = step_value_member(step);
            return HoistedElement {
                element: None,
                consumed_temps: vec![element],
            };
        }
    }

    // `use(item = _d);` as the first statement.
    if let Some(alias) = fold_inline_alias(body, &element) {
        return HoistedElement {
            element: Some(alias),
            consumed_temps: vec![element],
        };
    }

    HoistedElement {
        element: Some(element),
        consumed_temps: Vec::new(),
    }
}

/// When the body's first statement holds `alias = element` as an expression,
/// `element` is read nowhere else in the body, and that assignment is the
/// first mention of `alias` in the statement's evaluation order, replace the
/// assignment with `alias` and return it: binding `alias` in the loop head
/// gives the statement the same value, and every later read of `alias` in
/// the body already saw the assigned value.
fn fold_inline_alias(body: &mut BlockStmt, element: &Ident) -> Option<Ident> {
    let first = body.stmts.first()?;
    let mut finder = InlineAliasFinder {
        element: binding_key(element),
        mentions: Vec::new(),
        alias_assignments: Vec::new(),
        function_depth: 0,
        nested_alias_assignment: false,
    };
    first.visit_with(&mut finder);
    // An assignment inside a nested function runs when that function is
    // called, not in this statement's evaluation order.
    if finder.nested_alias_assignment {
        return None;
    }
    let [(alias, target_position)] = finder.alias_assignments.as_slice() else {
        return None;
    };
    let alias = alias.clone();
    if same_binding(&alias, element) {
        return None;
    }
    let element_key = binding_key(element);
    if finder
        .mentions
        .iter()
        .filter(|key| **key == element_key)
        .count()
        != 1
    {
        return None;
    }
    let alias_key = binding_key(&alias);
    if finder.mentions[..*target_position].contains(&alias_key) {
        return None;
    }
    if body.stmts[1..]
        .iter()
        .any(|stmt| stmt_mentions_binding(stmt, element))
    {
        return None;
    }
    let mut replacer = InlineAliasReplacer {
        element: element_key,
        alias: alias.clone(),
    };
    body.stmts[0].visit_mut_with(&mut replacer);
    Some(alias)
}

/// Identifier mentions of one statement in traversal (source) order, plus
/// each `alias = element` assignment with the position its target occupies
/// in that order.
struct InlineAliasFinder {
    element: BindingKey,
    mentions: Vec<BindingKey>,
    alias_assignments: Vec<(Ident, usize)>,
    /// Depth of nested functions around the node being visited.
    function_depth: usize,
    /// An `alias = element` assignment sits inside a nested function.
    nested_alias_assignment: bool,
}

impl Visit for InlineAliasFinder {
    fn visit_ident(&mut self, ident: &Ident) {
        self.mentions.push(binding_key(ident));
    }

    fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
        self.function_depth += 1;
        function.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        self.function_depth += 1;
        arrow.visit_children_with(self);
        self.function_depth -= 1;
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let Some(target) = assign_target_ident(assign) {
            if matches!(strip_parens(&assign.right), Expr::Ident(id)
                if id.sym == self.element.0 && id.ctxt == self.element.1)
            {
                if self.function_depth > 0 {
                    self.nested_alias_assignment = true;
                }
                self.alias_assignments.push((target, self.mentions.len()));
            }
        }
        assign.visit_children_with(self);
    }
}

struct InlineAliasReplacer {
    element: BindingKey,
    alias: Ident,
}

impl VisitMut for InlineAliasReplacer {
    fn visit_mut_function(&mut self, _: &mut swc_core::ecma::ast::Function) {}

    fn visit_mut_arrow_expr(&mut self, _: &mut swc_core::ecma::ast::ArrowExpr) {}

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Assign(assign) = expr {
            let is_alias_assign = assign_target_ident(assign)
                .is_some_and(|target| same_binding(&target, &self.alias))
                && matches!(strip_parens(&assign.right), Expr::Ident(id)
                    if id.sym == self.element.0 && id.ctxt == self.element.1);
            if is_alias_assign {
                *expr = Expr::Ident(self.alias.clone());
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }
}

/// The target of a leading `target = <right>;` statement whose right side
/// satisfies `right_matches`.
fn leading_assignment(body: &BlockStmt, right_matches: impl Fn(&Expr) -> bool) -> Option<Ident> {
    let Stmt::Expr(ExprStmt { expr, .. }) = body.stmts.first()? else {
        return None;
    };
    let Expr::Assign(assign) = strip_parens(expr) else {
        return None;
    };
    let target = assign_target_ident(assign)?;
    right_matches(&assign.right).then_some(target)
}

/// `for await (const item of iterable)` when `item` is private to the loop
/// (`let` if the body writes it), else `for await (item of iterable)`.
fn build_hoisted_element_for_await(
    body: BlockStmt,
    iterable: Box<Expr>,
    element: Ident,
    element_is_private: bool,
    span: swc_core::common::Span,
) -> Option<ForOfStmt> {
    let left = if element_is_private {
        let kind = if BindingUseIndex::collect_stmts(&body.stmts)
            .has_direct_write(&binding_key(&element))
        {
            VarDeclKind::Let
        } else {
            VarDeclKind::Const
        };
        ForHead::VarDecl(Box::new(swc_core::ecma::ast::VarDecl {
            span: DUMMY_SP,
            ctxt: Default::default(),
            kind,
            declare: false,
            decls: vec![swc_core::ecma::ast::VarDeclarator {
                span: DUMMY_SP,
                name: Pat::Ident(swc_core::ecma::ast::BindingIdent {
                    id: element,
                    type_ann: None,
                }),
                init: None,
                definite: false,
            }],
        }))
    } else {
        ForHead::Pat(Box::new(Pat::Ident(swc_core::ecma::ast::BindingIdent {
            id: element,
            type_ann: None,
        })))
    };
    let for_span = if span.lo.0 != 0 { span } else { DUMMY_SP };
    let mut for_of = ForOfStmt {
        span: for_span,
        is_await: true,
        left,
        right: iterable,
        body: Box::new(Stmt::Block(body)),
    };
    unwrap_single_block_body(&mut for_of);
    Some(for_of)
}

/// Every module use of `ident` sits inside `stmts`.
fn uses_are_within(ctx: &ForOfHelperContext, ident: &Ident, stmts: &[Stmt]) -> bool {
    let key = binding_key(ident);
    BindingUseIndex::collect_stmts(stmts).use_count(&key) == ctx.binding_uses.use_count(&key)
}

fn declared_idents(stmt: &Stmt) -> Vec<Ident> {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => var
            .decls
            .iter()
            .filter_map(|decl| pat_as_ident(&decl.name).map(|binding| binding.id.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Drop the declarations of protocol temporaries that an inner statement
/// list already folded into `for await`: uninitialized declarators whose
/// binding was proven private to that protocol. A `for await` head that
/// re-declares the element binding replaces its hoisted declaration.
pub(super) fn remove_orphaned_temp_declarations(stmts: &mut Vec<Stmt>, ctx: &ForOfHelperContext) {
    let orphaned = ctx.orphaned_protocol_temps.borrow();
    if orphaned.is_empty() {
        return;
    }
    stmts.retain_mut(|stmt| {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return true;
        };
        var.decls.retain(|decl| {
            !(decl.init.is_none()
                && pat_as_ident(&decl.name)
                    .is_some_and(|binding| orphaned.contains(&binding_key(&binding.id))))
        });
        !var.decls.is_empty()
    });
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
    let Stmt::Block(body) = &*for_stmt.body else {
        return None;
    };
    let mut body = body.clone();
    let mut shape = match for_stmt.test.as_deref() {
        Some(test) => parse_loop_test(test, &iterator)?,
        None => parse_head_statements(&mut body, &iterator)?,
    };
    parse_loop_update(for_stmt, &mut shape, &preamble_flags)?;

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
        body,
    })
}

/// A state-machine decode of Terser-compressed TypeScript output puts the
/// loop test in the body: `step = await it.next(); if (done = step.done)
/// break;` (or `if (step.done) break;`) leads the body and the `for` test is
/// empty. Consume both statements.
fn parse_head_statements(body: &mut BlockStmt, iterator: &Ident) -> Option<LoopTest> {
    let [Stmt::Expr(next_stmt), Stmt::If(guard), ..] = body.stmts.as_slice() else {
        return None;
    };
    let Expr::Assign(next_assign) = strip_parens(&next_stmt.expr) else {
        return None;
    };
    let step = assign_target_ident(next_assign)?;
    if !is_awaited_next_call(&next_assign.right, iterator) {
        return None;
    }
    if guard.alt.is_some() || !matches!(single_stmt(&guard.cons)?, Stmt::Break(_)) {
        return None;
    }
    let done_temp = match strip_parens(&guard.test) {
        Expr::Assign(done_assign) => {
            let done_temp = assign_target_ident(done_assign)?;
            if !is_done_member_of(&done_assign.right, &step) {
                return None;
            }
            Some(done_temp)
        }
        test if is_done_member_of(test, &step) => None,
        _ => return None,
    };
    body.stmts.drain(..2);
    Some(LoopTest {
        step,
        abrupt_flag: None,
        normal_flag: None,
        first_flag: None,
        done_temp,
        value_temp: None,
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

/// TypeScript 5 clears its `first` flag right after reading the value
/// (`value = step.value; first = false;`). Consume the clear so the value
/// copy is the first body statement; `take_hoisted_element` folds that copy
/// into the loop binding.
fn strip_body_protocol_prefix(shape: &mut LoopShape, _protocol_idents: &mut [Ident]) -> Option<()> {
    let Some(first) = shape.first_flag.clone() else {
        return Some(());
    };
    let clears_first = |stmt: &Stmt| {
        let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
            return false;
        };
        let Expr::Assign(assign) = strip_parens(expr) else {
            return false;
        };
        assign_target_ident(assign).is_some_and(|target| same_binding(&first, &target))
            && matches!(strip_parens(&assign.right), Expr::Lit(Lit::Bool(b)) if !b.value)
    };
    // The clear is the first or second statement, after the value copy.
    let position = shape.body.stmts.iter().take(2).position(clears_first)?;
    shape.body.stmts.remove(position);
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
            // `{ error: err }`, or `{ error }` once the catch parameter itself
            // is named `error`.
            Expr::Object(object) => {
                let [PropOrSpread::Prop(prop)] = object.props.as_slice() else {
                    return None;
                };
                let captured = match prop.as_ref() {
                    Prop::KeyValue(kv) => {
                        matches!(&kv.key, PropName::Ident(key) if key.sym.as_ref() == "error")
                            && is_ident_key(strip_parens(&kv.value), &param)
                    }
                    Prop::Shorthand(id) => {
                        id.sym.as_ref() == "error" && is_ident_key(&Expr::Ident(id.clone()), &param)
                    }
                    _ => false,
                };
                if !captured {
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

/// `ident` is mentioned inside a function or arrow nested in `stmts`.
fn nested_function_mentions_binding(stmts: &[Stmt], ident: &Ident) -> bool {
    struct NestedMentionFinder {
        key: BindingKey,
        function_depth: usize,
        found: bool,
    }

    impl Visit for NestedMentionFinder {
        fn visit_ident(&mut self, ident: &Ident) {
            if self.function_depth > 0 && ident.sym == self.key.0 && ident.ctxt == self.key.1 {
                self.found = true;
            }
        }

        fn visit_function(&mut self, function: &swc_core::ecma::ast::Function) {
            self.function_depth += 1;
            function.visit_children_with(self);
            self.function_depth -= 1;
        }

        fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
            self.function_depth += 1;
            arrow.visit_children_with(self);
            self.function_depth -= 1;
        }
    }

    let mut finder = NestedMentionFinder {
        key: binding_key(ident),
        function_depth: 0,
        found: false,
    };
    for stmt in stmts {
        stmt.visit_with(&mut finder);
    }
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
