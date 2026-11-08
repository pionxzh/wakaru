use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    ArrayLit, ArrowExpr, ArrowFunctionBody, AssignExpr, AssignOp, AssignTarget, BindingIdent,
    CallExpr, Callee, Constructor, Decl, Expr, ExprOrSpread, ExprStmt, FnDecl, Function,
    FunctionBody, Ident, Lit, MemberExpr, MemberProp, Module, ModuleItem, NewExpr,
    ParamOrTsParamProp, Pat, SimpleAssignTarget, Stmt, UpdateExpr, VarDeclKind, VarDeclarator,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::arg_rest::find_rest_array_copy_proof;
use super::binding_facts::TempIsolation;
use super::construct_helper::ConstructHelpers;
use super::dead_decls::remove_consumed_uninitialized_decls;
use super::decl_utils::same_ident;
use super::expr_utils::{exprs_structurally_equal, is_unresolved_ident, is_unresolved_undefined};
use super::un_array_concat_spread::ClosureRuntime;
use super::RewriteLevel;

use crate::analysis::binding_id;
use crate::analysis::binding_uses::{BindingId, BindingUseIndex};
use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;

/// Rewrites `fn.apply(thisArg, args)` to a spread call `fn(...args)`.
///
/// `apply` and spread read their argument list differently. `apply` reads
/// `length` and indexes of any array-like and treats `null`/`undefined` as no
/// arguments; spread iterates and throws on both. At `standard` the argument
/// must therefore be proven to read the same both ways (see
/// `ProvenArgumentLists`); an unknown value converts only at `aggressive`,
/// under the `apply_arguments_are_arrays` assumption. A `null` receiver is
/// `aggressive` too (`apply_null_receiver_unobserved`): a strict callee sees
/// `this === null` through `apply` but `undefined` in a bare call.
///
/// An argument that carries a lowerer's spread helper (a recovered
/// `[...xs]`, or Closure's `arrayFromIterable`) proves the call was a spread
/// call, so the receiver the lowerer chose recovers to the source call at
/// every level (`lowered_spread_call_source_semantics`): Closure's `null` and
/// its unmemoized member-chain receiver, and swc's `this` on a call-result
/// callee. The named `$jscomp.arrayFromIterable(xs)` is dropped from the
/// spread; a renamed helper stays (see `ClosureRuntime`).
///
/// A lowered spread construct ends as a construct of a bound function,
/// which becomes `new F(...args)` (see `bound_construct`). Its argument list
/// takes the same proof: TypeScript's `F.bind.apply(F, list)` reaches it
/// through the `apply` rewrite above, and Babel 6's and Closure's
/// `Function.prototype.bind.apply(F, [null].concat(…))` once the list is an
/// array literal, after the concat and helper rules made it one, or directly
/// when every concat argument is an array literal or a fresh Array (Babel 6's
/// `Array.prototype.slice.call(arguments)`; see `concat_list_elements`).
///
/// Babel 7's and swc's `_construct(C, list)` helper, proven by its body
/// (`ConstructHelpers`), constructs `C` with `list` read as an array-like, so
/// a two-argument call becomes `new C(...list)` under the same proof.
pub struct UnArgumentSpread {
    unresolved_mark: Mark,
    level: RewriteLevel,
    isolation: TempIsolation,
    consumed_uninitialized_bindings: HashSet<BindingId>,
    /// The module contains a `with` statement or a direct eval.
    dynamic_scope: bool,
    proven: ProvenArgumentLists,
    construct_helpers: ConstructHelpers,
    rewrote_construct_helper_call: bool,
}

impl UnArgumentSpread {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
            isolation: TempIsolation::default(),
            consumed_uninitialized_bindings: HashSet::default(),
            dynamic_scope: false,
            proven: ProvenArgumentLists::default(),
            construct_helpers: ConstructHelpers::default(),
            rewrote_construct_helper_call: false,
        }
    }

    /// `apply_arguments_are_arrays`: only `aggressive` spreads an argument
    /// list that is not proven to be an Array or the `arguments` object.
    fn argument_list_converts(&self, expr: &Expr) -> bool {
        self.level >= RewriteLevel::Aggressive
            || self
                .proven
                .proves(expr, self.unresolved_mark, self.dynamic_scope)
    }

    /// Visit a body's statements in order. A positional candidate (a `var`
    /// array or an `arguments` copy) holds an Array only from its ready index
    /// on, so it is activated as the walk passes that index.
    fn visit_body_stmts(&mut self, stmts: &mut Vec<Stmt>, candidates: Vec<PositionalCandidate>) {
        for (index, stmt) in stmts.iter_mut().enumerate() {
            self.proven.activate(&candidates, index);
            stmt.visit_mut_with(self);
        }
        self.proven.deactivate(&candidates);
        self.rewrite_split_memoized_applies(stmts, &candidates);
        self.proven.deactivate(&candidates);
    }

    /// `fn(...[a, ...xs])` → `fn(a, ...xs)` for a hole-free literal or a
    /// concat `concat_list_elements` accepts, and
    /// `fn(...$jscomp.arrayFromIterable(xs))` → `fn(...xs)`: the named helper
    /// returns an Array `xs` as is and otherwise iterates it, as spread does.
    fn finish_spread_call(&self, expr: &mut Expr) {
        let Expr::Call(call) = expr else {
            return;
        };
        let [arg] = call.args.as_mut_slice() else {
            return;
        };
        if arg.spread.is_none() {
            return;
        }
        if let Some(source) = self
            .proven
            .closure
            .as_ref()
            .and_then(|closure| closure.array_from_iterable_source(&arg.expr))
        {
            *arg.expr = source.clone();
            return;
        }
        let arg = call.args.remove(0);
        call.args = self.spread_arguments(arg.expr);
    }

    fn visit_function_body(&mut self, body: &mut FunctionBody, fixed_param_count: Option<usize>) {
        let candidates = self.proven.body_candidates(
            &body.stmts,
            fixed_param_count
                .and_then(|count| find_rest_array_copy_proof(body, count, self.unresolved_mark)),
        );
        self.visit_body_stmts(&mut body.stmts, candidates);
    }

    fn rewrite_split_memoized_applies(
        &mut self,
        stmts: &mut Vec<Stmt>,
        candidates: &[PositionalCandidate],
    ) {
        if self.level < RewriteLevel::Standard {
            return;
        }

        let mut old = std::mem::take(stmts);
        let mut index = 0;
        while index < old.len() {
            if index + 1 < old.len() {
                // The apply statement is the second of the pair.
                self.proven.activate(candidates, index + 1);
                if let Some(stmt) = self.try_rewrite_split_memoized_apply(&old[index..index + 2]) {
                    stmts.push(stmt);
                    index += 2;
                    continue;
                }
            }

            stmts.push(std::mem::replace(
                &mut old[index],
                Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: DUMMY_SP }),
            ));
            index += 1;
        }
    }
}

impl Default for UnArgumentSpread {
    fn default() -> Self {
        Self::new(Mark::new(), RewriteLevel::Standard)
    }
}

/// A memoized receiver `(t = expr)` loses its assignment only when
/// `TempIsolation` accepts the rewrite that drops it; otherwise the
/// assignment stays on the callee object, where it still runs before the
/// arguments. So each rewrite is tried with the assignment dropped first.
const RECEIVER_CHOICES: [bool; 2] = [true, false];

impl VisitMut for UnArgumentSpread {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if self.level < RewriteLevel::Standard {
            return;
        }
        if !contains_apply_call(module) {
            return;
        }
        self.dynamic_scope = super::eval_utils::has_dynamic_scope_construct(module);
        self.isolation = TempIsolation::collect(module);
        self.proven =
            ProvenArgumentLists::collect(module, self.unresolved_mark, self.dynamic_scope);
        self.consumed_uninitialized_bindings.clear();
        self.construct_helpers = if self.dynamic_scope {
            ConstructHelpers::default()
        } else {
            ConstructHelpers::collect(module, self.unresolved_mark)
        };
        self.rewrote_construct_helper_call = false;
        module.visit_mut_children_with(self);
        remove_consumed_uninitialized_decls(module, &self.consumed_uninitialized_bindings);
        if self.rewrote_construct_helper_call {
            // A helper still called (the three-argument `_wrapNativeSuper`
            // form, an unproven list) keeps itself and what it calls.
            let candidates = self
                .construct_helpers
                .helpers
                .iter()
                .chain(&self.construct_helpers.dependencies)
                .cloned()
                .collect();
            super::helper_matcher::remove_unused_helper_declarations(module, &candidates);
        }
    }

    fn visit_mut_module_items(&mut self, items: &mut Vec<ModuleItem>) {
        let candidates = self.proven.body_candidates(items, None);
        for (index, item) in items.iter_mut().enumerate() {
            self.proven.activate(&candidates, index);
            item.visit_mut_with(self);
        }
        self.proven.deactivate(&candidates);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        self.visit_body_stmts(stmts, Vec::new());
    }

    // Function bodies are visited here, not through `visit_mut_function_body`,
    // so the `arguments` copy proof can see the parameter count.
    fn visit_mut_function(&mut self, function: &mut Function) {
        function.decorators.visit_mut_with(self);
        function.params.visit_mut_with(self);
        if let Some(body) = &mut function.body {
            self.visit_function_body(body, Some(function.params.len()));
        }
    }

    fn visit_mut_constructor(&mut self, constructor: &mut Constructor) {
        constructor.key.visit_mut_with(self);
        constructor.params.visit_mut_with(self);
        let fixed_param_count = constructor.params.len();
        if let Some(body) = &mut constructor.body {
            self.visit_function_body(body, Some(fixed_param_count));
        }
    }

    fn visit_mut_arrow_expr(&mut self, arrow: &mut ArrowExpr) {
        arrow.params.visit_mut_with(self);
        match arrow.body.as_mut() {
            // Arrows have no own `arguments`, so there is no copy to prove.
            ArrowFunctionBody::FunctionBody(body) => self.visit_function_body(body, None),
            ArrowFunctionBody::Expr(expr) => expr.visit_mut_with(self),
        }
    }

    // A hoisted function declaration can run before a positional candidate is
    // initialized, so its body sees only position-independent proofs.
    fn visit_mut_fn_decl(&mut self, decl: &mut FnDecl) {
        self.proven.fn_decl_depth += 1;
        decl.visit_mut_children_with(self);
        self.proven.fn_decl_depth -= 1;
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        if let Expr::New(new) = &*expr {
            if let Some(rewritten) = self.bound_construct(new) {
                *expr = rewritten;
            }
            return;
        }

        let Expr::Call(call) = &*expr else {
            return;
        };
        if let Some(rewritten) = self.construct_helper_call(call) {
            *expr = rewritten;
            self.rewrote_construct_helper_call = true;
            return;
        }
        if !is_apply_call(call) || !self.argument_list_converts(&call.args[1].expr) {
            return;
        }
        let lowered_spread = self
            .proven
            .lowered_spread_evidence(&call.args[1].expr, self.dynamic_scope);
        let call = call.clone();

        let choices: &[bool] = if expr_memoized_apply_assign(&call).is_some() {
            &RECEIVER_CHOICES
        } else {
            &[false]
        };
        for &drop_receiver in choices {
            let Ok(mut rewritten) = try_convert_apply(
                call.clone(),
                self.unresolved_mark,
                drop_receiver,
                self.dynamic_scope,
                self.level >= RewriteLevel::Aggressive || lowered_spread,
                lowered_spread,
            ) else {
                return;
            };
            self.finish_spread_call(&mut rewritten);
            if self.isolation.accept_expr_rewrite(
                expr,
                &rewritten,
                &mut self.consumed_uninitialized_bindings,
            ) {
                *expr = rewritten;
                return;
            }
        }
    }
}

impl UnArgumentSpread {
    /// `_construct(C, list)` → `new C(...list)`, with the list's proof.
    fn construct_helper_call(&self, call: &CallExpr) -> Option<Expr> {
        let (target, list) = self.construct_helpers.two_argument_call(call)?;
        if !self.argument_list_converts(&list.expr) {
            return None;
        }
        Some(Expr::New(NewExpr {
            span: call.span,
            ctxt: call.ctxt,
            callee: target.expr.clone(),
            args: Some(self.spread_arguments(list.expr.clone())),
            type_args: None,
        }))
    }

    /// `new (F.bind(t, ...args))()` → `new F(...args)`, and
    /// `new (Function.prototype.bind.apply(F, [t, ...args]))()` likewise.
    ///
    /// [[Construct]] of a bound function ignores the bound `this` and passes
    /// the target as `new.target`, so the two construct the same object when
    /// `t` has no side effect and the outer call adds no arguments.
    /// `Function.prototype.bind` (or `Function.bind`) on the global `Function`
    /// is the intrinsic under the execution baseline; `F.bind` is the
    /// intrinsic only under `intrinsic_function_bind`, since `F` may define
    /// its own `bind`. The list of the `apply` form must be a hole-free array
    /// literal, or a concat `concat_list_elements` reads as one (Babel 6's
    /// `[null].concat(Array.prototype.slice.call(arguments))`): an unknown
    /// list cannot be split into its bound `this` and arguments.
    fn bound_construct(&self, new: &NewExpr) -> Option<Expr> {
        if new.args.as_ref().is_some_and(|args| !args.is_empty()) {
            return None;
        }
        let Expr::Call(call) = strip_parens(&new.callee) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) = strip_parens(callee)
        else {
            return None;
        };
        let (target, args) = match prop.sym.as_ref() {
            "bind" => {
                let (bound_this, args) = call.args.split_first()?;
                if bound_this.spread.is_some() || !self.is_ignored_bound_this(&bound_this.expr) {
                    return None;
                }
                (obj.clone(), args.to_vec())
            }
            "apply" if self.is_intrinsic_bind(obj) => {
                let [target, list] = call.args.as_slice() else {
                    return None;
                };
                if target.spread.is_some() || list.spread.is_some() {
                    return None;
                }
                let elems = match strip_parens(&list.expr) {
                    Expr::Array(list) => hole_free_elements(list)?,
                    list => self.concat_list_elements(list)?,
                };
                let (bound_this, rest) = elems.split_first()?;
                if bound_this.spread.is_some() || !self.is_ignored_bound_this(&bound_this.expr) {
                    return None;
                }
                (target.expr.clone(), rest.to_vec())
            }
            _ => return None,
        };
        let mut args = args;
        self.spread_arguments_directly(&mut args);
        Some(Expr::New(NewExpr {
            span: new.span,
            ctxt: new.ctxt,
            callee: target,
            args: Some(args),
            type_args: new.type_args.clone(),
        }))
    }

    /// The elements of `[e…].concat(a…)` as one list, when the receiver and
    /// every array-literal argument are hole-free and every other argument
    /// is a fresh Array (`proves_fresh_array`), spread in place.
    ///
    /// Concat appends the elements of an Array argument
    /// (`IsConcatSpreadable`), and a fresh Array has no own
    /// `Symbol.isConcatSpreadable` to say otherwise; its holes stay holes,
    /// which a list read by `apply` or by iteration reads as `undefined`
    /// either way. A binding is not taken even when proven: code holding it
    /// may have set its `Symbol.isConcatSpreadable`.
    fn concat_list_elements(&self, expr: &Expr) -> Option<Vec<ExprOrSpread>> {
        let Expr::Call(call) = strip_parens(expr) else {
            return None;
        };
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) = strip_parens(callee)
        else {
            return None;
        };
        if prop.sym != "concat" {
            return None;
        }
        let Expr::Array(receiver) = strip_parens(obj) else {
            return None;
        };
        let mut elems = hole_free_elements(receiver)?;
        for arg in &call.args {
            if arg.spread.is_some() {
                return None;
            }
            match strip_parens(&arg.expr) {
                Expr::Array(array) => elems.extend(hole_free_elements(array)?),
                expr if self.proven.proves_fresh_array(
                    expr,
                    self.unresolved_mark,
                    self.dynamic_scope,
                ) =>
                {
                    elems.push(ExprOrSpread {
                        spread: Some(DUMMY_SP),
                        expr: arg.expr.clone(),
                    })
                }
                _ => return None,
            }
        }
        Some(elems)
    }

    /// The arguments of a spread call or construct of `list`: the elements
    /// of a hole-free array literal or of a concat `concat_list_elements`
    /// accepts, else `...list`.
    fn spread_arguments(&self, list: Box<Expr>) -> Vec<ExprOrSpread> {
        let mut args = match self.concat_list_elements(&list) {
            Some(elems) => elems,
            None => args_from_apply_arg(list),
        };
        self.spread_arguments_directly(&mut args);
        args
    }

    /// `...Array.prototype.slice.call(arguments)` → `...arguments`, also
    /// with a literal `0` start.
    ///
    /// Spreading the copy and spreading the `arguments` object read the
    /// same values: `Get` of each index below `length`, `undefined` for a
    /// missing one, under the baseline that already lets the argument list
    /// `arguments` itself convert (`ProvenArgumentLists`). The dropped reads
    /// of `Array.prototype.slice` and `call` are intrinsic (`stable_builtins`).
    fn spread_arguments_directly(&self, args: &mut [ExprOrSpread]) {
        if self.dynamic_scope || self.proven.arguments_written {
            return;
        }
        for arg in args {
            if arg.spread.is_none() {
                continue;
            }
            let Some(arguments) = arguments_copy_source(&arg.expr, self.unresolved_mark).cloned()
            else {
                continue;
            };
            *arg.expr = Expr::Ident(arguments);
        }
    }

    /// A bound `this` whose evaluation has no effect: `null`, `void 0`, or
    /// the global `undefined` outside a module with dynamic scope.
    fn is_ignored_bound_this(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Lit(Lit::Null(_)) => true,
            Expr::Ident(_) if self.dynamic_scope => false,
            expr => is_unresolved_undefined(expr, self.unresolved_mark),
        }
    }

    /// `Function.prototype.bind` or `Function.bind` on the global `Function`.
    fn is_intrinsic_bind(&self, expr: &Expr) -> bool {
        if self.dynamic_scope {
            return false;
        }
        let Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) = strip_parens(expr)
        else {
            return false;
        };
        if prop.sym != "bind" {
            return false;
        }
        let is_function = |expr: &Expr| matches!(strip_parens(expr), Expr::Ident(id) if is_unresolved_ident(id, "Function", self.unresolved_mark));
        match strip_parens(obj) {
            Expr::Member(MemberExpr {
                obj,
                prop: MemberProp::Ident(prop),
                ..
            }) => prop.sym == "prototype" && is_function(obj),
            obj => is_function(obj),
        }
    }

    fn try_rewrite_split_memoized_apply(&mut self, pair: &[Stmt]) -> Option<Stmt> {
        let matched = match_split_memoized_apply(&pair[0], &pair[1])?;
        if !self.argument_list_converts(&matched.apply_call.args[1].expr) {
            return None;
        }
        let choices: &[bool] = match matched.receiver {
            SplitReceiver::Memoized(_) => &RECEIVER_CHOICES,
            SplitReceiver::Direct => &[false],
        };
        for &drop_receiver in choices {
            let stmt = build_split_memoized_apply(&matched, drop_receiver);
            if self.isolation.accept_stmts_rewrite(
                pair,
                std::slice::from_ref(&stmt),
                &mut self.consumed_uninitialized_bindings,
            ) {
                return Some(stmt);
            }
        }
        None
    }
}

/// `X.apply(thisArg, args)` with exactly two non-spread arguments.
fn is_apply_call(call: &CallExpr) -> bool {
    let Callee::Expr(callee) = &call.callee else {
        return false;
    };
    matches!(
        callee.as_ref(),
        Expr::Member(MemberExpr { prop: MemberProp::Ident(prop), .. }) if prop.sym.as_ref() == "apply"
    ) && call.args.len() == 2
        && call.args.iter().all(|arg| arg.spread.is_none())
}

fn try_convert_apply(
    call: CallExpr,
    unresolved_mark: Mark,
    drop_receiver: bool,
    dynamic_scope: bool,
    allow_null_receiver: bool,
    lowered_spread: bool,
) -> Result<Expr, CallExpr> {
    // callee must be a member expression ending in `.apply`
    let callee_member = match &call.callee {
        Callee::Expr(e) => match e.as_ref() {
            Expr::Member(m) => m,
            _ => return Err(call),
        },
        _ => return Err(call),
    };

    // Check that the property is `apply`
    match &callee_member.prop {
        MemberProp::Ident(ident_name) if ident_name.sym.as_ref() == "apply" => {}
        _ => return Err(call),
    }

    // We need exactly 2 arguments
    if call.args.len() != 2 {
        return Err(call);
    }

    // Check for spread on either arg – we don't handle those
    if call.args[0].spread.is_some() || call.args[1].spread.is_some() {
        return Err(call);
    }

    let first_arg = call.args[0].expr.as_ref();
    let callee_obj = callee_member.obj.as_ref();

    // Pattern 1: fn.apply(null/undefined, arg2) → fn(...arg2)
    // Only applies when the callee object is NOT itself a member expression
    // (i.e., the callee is just `fn`, not `obj.fn`)
    // Actually per the JS spec, for plain fn.apply(null/undefined) we convert regardless.
    // But if it's obj.fn.apply(obj, ...) we match pattern 2 instead.
    // Determine which pattern applies:

    // Pattern 2: obj.fn.apply(obj, arg2) → obj.fn(...arg2)
    // The callee's object is a member expression AND first arg equals the outer object.
    // e.g. callee = obj.fn.apply, callee_obj = obj.fn (Member), first_arg should = obj
    if let Expr::Member(callee_member_obj) = callee_obj {
        // The non-memoized same-receiver form accepts only an identifier or
        // `this` receiver. A member-chain receiver (`root.child.method.apply(
        // root.child, args)`) is read twice by the input and once by the
        // output, so a getter's evaluation count would change. Babel emits
        // the bare form only for plain identifiers and `this`; it memoizes
        // member receivers, which the memoized paths below handle.
        if matches!(first_arg, Expr::Ident(_) | Expr::This(_))
            && exprs_structurally_equal(first_arg, &callee_member_obj.obj)
        {
            return Ok(make_spread_call(call));
        }
        if let Some(assign) = memoized_receiver_assign(&callee_member_obj.obj, first_arg) {
            let receiver = memoized_receiver(assign, drop_receiver);
            return Ok(make_spread_call_with_member_receiver(call, receiver));
        }
        // Closure Compiler lowers `root.a.m(...xs)` to the unmemoized
        // `root.a.m.apply(root.a, …)`, reading `root.a` twice where the source
        // read it once; with spread-helper evidence, the source call is the
        // recovery.
        if lowered_spread
            && is_static_member_chain(first_arg)
            && exprs_structurally_equal(first_arg, &callee_member_obj.obj)
        {
            return Ok(make_spread_call(call));
        }
        // obj.fn.apply(null/undefined, ...) — Babel spread artifact for standalone
        // function calls on module namespaces (e.g. `r.applyMiddleware.apply(void 0, d)`).
        // Not converted here because it changes `this` from undefined to obj.
        // The proper fix is namespace import decomposition (r.fn → fn), after which
        // Pattern 1 (simple ident) handles it.
        return Err(call);
    }

    // Pattern 1: callee obj is not a member expression, first arg must be null/undefined.
    // Inside a `with` body the bare call `fn(...)` passes the object as `this`
    // when `fn` resolves through it, and a direct eval can rebind `undefined`;
    // such a module keeps the `apply` call (docs/rewrite-assumptions.md,
    // dynamic-scope skip).
    if dynamic_scope {
        return Err(call);
    }
    // An `undefined` receiver reads the same both ways: a sloppy callee gets
    // the global object from `apply` and from a bare call alike. A strict
    // callee sees `this === null` only through `apply`.
    if is_unresolved_undefined(first_arg, unresolved_mark)
        || (allow_null_receiver && matches!(first_arg, Expr::Lit(Lit::Null(_))))
    {
        return Ok(make_spread_call(call));
    }
    // swc lowers `make()(...xs)` to `make().apply(this, …)`; the source call
    // had no receiver. Without spread-helper evidence, `getFn().apply(this,
    // args)` is a common hand-written forward.
    if lowered_spread
        && matches!(first_arg, Expr::This(_))
        && matches!(strip_parens(callee_obj), Expr::Call(_))
    {
        return Ok(make_spread_call(call));
    }

    Err(call)
}

/// A matched `method = <receiver>.prop; method.apply(thisArg, args)` pair.
struct SplitMemoizedApply<'a> {
    member: &'a MemberExpr,
    apply_call: &'a CallExpr,
    receiver: SplitReceiver<'a>,
}

enum SplitReceiver<'a> {
    /// `method = obj.prop; method.apply(obj, args)` with an identifier or
    /// `this` receiver, which may be read twice by the input and once by the
    /// output without changing a getter's evaluation count. Member receivers
    /// arrive memoized into a temp and take the other arm.
    Direct,
    /// `method = (receiver = expr).prop; method.apply(receiver, args)`.
    Memoized(&'a AssignExpr),
}

fn match_split_memoized_apply<'a>(
    method_stmt: &'a Stmt,
    apply_stmt: &'a Stmt,
) -> Option<SplitMemoizedApply<'a>> {
    let (method_temp, member) = memoized_method_assignment(method_stmt)?;
    let apply_call = expr_stmt_call(apply_stmt)?;

    if apply_call.args.len() != 2
        || apply_call.args[0].spread.is_some()
        || apply_call.args[1].spread.is_some()
    {
        return None;
    }

    let apply_member = match &apply_call.callee {
        Callee::Expr(callee) => match callee.as_ref() {
            Expr::Member(member) => member,
            _ => return None,
        },
        _ => return None,
    };
    if !matches!(&apply_member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "apply") {
        return None;
    }
    if !matches!(apply_member.obj.as_ref(), Expr::Ident(id) if same_ident(id, &method_temp)) {
        return None;
    }

    let first_arg = apply_call.args[0].expr.as_ref();
    let receiver = if matches!(first_arg, Expr::Ident(_) | Expr::This(_))
        && exprs_structurally_equal(first_arg, &member.obj)
    {
        SplitReceiver::Direct
    } else {
        let assign = memoized_receiver_assign(&member.obj, first_arg)?;
        // `method = (method = output).push` writes the function over the
        // receiver, so `thisArg` is the function, not `output`.
        if memoized_assign_target(assign).is_some_and(|target| same_ident(target, &method_temp)) {
            return None;
        }
        SplitReceiver::Memoized(assign)
    };

    Some(SplitMemoizedApply {
        member,
        apply_call,
        receiver,
    })
}

/// `receiver.prop(...args)` for a matched pair, with a memoized receiver's
/// assignment dropped or kept.
fn build_split_memoized_apply(matched: &SplitMemoizedApply<'_>, drop_receiver: bool) -> Stmt {
    let receiver = match matched.receiver {
        SplitReceiver::Direct => matched.member.obj.clone(),
        SplitReceiver::Memoized(assign) => memoized_receiver(assign, drop_receiver),
    };

    let apply_call = matched.apply_call;
    let callee = Expr::Member(MemberExpr {
        span: matched.member.span,
        obj: receiver,
        prop: matched.member.prop.clone(),
    });

    Stmt::Expr(ExprStmt {
        span: apply_call.span,
        expr: Box::new(Expr::Call(CallExpr {
            span: apply_call.span,
            ctxt: apply_call.ctxt,
            callee: Callee::Expr(Box::new(callee)),
            args: args_from_apply_arg(apply_call.args[1].expr.clone()),
            type_args: apply_call.type_args.clone(),
        })),
    })
}

/// The `(t = expr)` of `(t = expr).fn.apply(t, args)`, the shape
/// `try_convert_apply` rewrites through `memoized_receiver_assign`.
fn expr_memoized_apply_assign(call: &CallExpr) -> Option<&AssignExpr> {
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(apply_member) = callee.as_ref() else {
        return None;
    };
    if !matches!(&apply_member.prop, MemberProp::Ident(prop) if prop.sym.as_ref() == "apply") {
        return None;
    }
    if call.args.len() != 2 || call.args[0].spread.is_some() || call.args[1].spread.is_some() {
        return None;
    }
    let Expr::Member(fn_member) = apply_member.obj.as_ref() else {
        return None;
    };
    memoized_receiver_assign(&fn_member.obj, &call.args[0].expr)
}

fn memoized_method_assignment(stmt: &Stmt) -> Option<(Ident, &MemberExpr)> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Assign(assign) = expr_stmt.expr.as_ref() else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(method_temp)) = &assign.left else {
        return None;
    };
    let Expr::Member(member) = assign.right.as_ref() else {
        return None;
    };
    Some((method_temp.id.clone(), member))
}

fn expr_stmt_call(stmt: &Stmt) -> Option<&CallExpr> {
    let Stmt::Expr(expr_stmt) = stmt else {
        return None;
    };
    let Expr::Call(call) = expr_stmt.expr.as_ref() else {
        return None;
    };
    Some(call)
}

/// The elements of an array literal without holes.
fn hole_free_elements(array: &ArrayLit) -> Option<Vec<ExprOrSpread>> {
    array.elems.iter().cloned().collect()
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

/// Build `fn(...secondArg)` from the original `.apply(thisArg, secondArg)` call.
fn make_spread_call(call: CallExpr) -> Expr {
    // Consume the call
    let CallExpr {
        span,
        ctxt,
        callee,
        mut args,
        type_args,
    } = call;

    // callee is `fn.apply` – we want just `fn`
    let Callee::Expr(callee_box) = callee else {
        unreachable!()
    };
    let Expr::Member(member) = *callee_box else {
        unreachable!()
    };
    let fn_expr = member.obj;

    // second arg becomes the spread argument
    let second_arg = args.remove(1).expr;

    Expr::Call(CallExpr {
        span,
        ctxt,
        callee: Callee::Expr(fn_expr),
        args: vec![ExprOrSpread {
            spread: Some(DUMMY_SP),
            expr: second_arg,
        }],
        type_args,
    })
}

fn memoized_receiver_assign<'a>(
    receiver_expr: &'a Expr,
    first_arg: &Expr,
) -> Option<&'a AssignExpr> {
    let receiver_expr = strip_parens(receiver_expr);
    let Expr::Assign(assign) = receiver_expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let target = memoized_assign_target(assign)?;
    if !matches!(first_arg, Expr::Ident(id) if id.sym == target.sym && id.ctxt == target.ctxt) {
        return None;
    }
    Some(assign)
}

fn memoized_assign_target(assign: &AssignExpr) -> Option<&Ident> {
    match &assign.left {
        AssignTarget::Simple(SimpleAssignTarget::Ident(target)) => Some(&target.id),
        _ => None,
    }
}

/// The callee object for a memoized receiver. The callee object is
/// evaluated before the arguments, so keeping the whole assignment there
/// preserves the write for readers in the arguments or later code.
fn memoized_receiver(assign: &AssignExpr, drop_assignment: bool) -> Box<Expr> {
    if drop_assignment {
        assign.right.clone()
    } else {
        Box::new(Expr::Assign(assign.clone()))
    }
}

fn make_spread_call_with_member_receiver(mut call: CallExpr, receiver: Box<Expr>) -> Expr {
    if let Callee::Expr(callee) = &mut call.callee {
        if let Expr::Member(apply_member) = callee.as_mut() {
            if let Expr::Member(fn_member) = apply_member.obj.as_mut() {
                fn_member.obj = receiver;
            }
        }
    }
    make_spread_call(call)
}

/// `a.b.c` / `this.a`: identifier or `this` root, static property names.
fn is_static_member_chain(expr: &Expr) -> bool {
    match strip_parens(expr) {
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(_),
            ..
        }) => {
            matches!(strip_parens(obj), Expr::Ident(_) | Expr::This(_))
                || is_static_member_chain(obj)
        }
        _ => false,
    }
}

/// An `apply` call, or a construct of a `bind` call (`bound_construct`).
fn contains_apply_call(module: &Module) -> bool {
    struct Finder(bool);
    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.0 {
                return;
            }
            if is_apply_call(call) {
                self.0 = true;
                return;
            }
            call.visit_children_with(self);
        }

        fn visit_new_expr(&mut self, new: &NewExpr) {
            if self.0 {
                return;
            }
            if let Expr::Call(CallExpr {
                callee: Callee::Expr(callee),
                ..
            }) = strip_parens(&new.callee)
            {
                if matches!(strip_parens(callee), Expr::Member(MemberExpr { prop: MemberProp::Ident(prop), .. }) if prop.sym == "bind")
                {
                    self.0 = true;
                    return;
                }
            }
            new.visit_children_with(self);
        }
    }
    let mut finder = Finder(false);
    module.visit_with(&mut finder);
    finder.0
}

/// A binding that holds an Array from body item `ready` on: a `var`
/// initialized to an Array, a declaration whose Array proof depends on
/// another binding, or an `arguments` copy filled by a loop.
struct PositionalCandidate {
    binding: BindingId,
    ready: usize,
    /// The initializer is an Array only if this binding holds one when the
    /// candidate becomes ready (`var c = t.map(f)`).
    depends_on: Option<Ident>,
}

/// Array methods that return a new Array when called on a plain Array
/// (`ArraySpeciesCreate` with the intrinsic constructor, `stable_builtins`).
const ARRAY_RESULT_METHODS: [&str; 6] = ["concat", "filter", "flat", "flatMap", "map", "slice"];

/// Argument lists that `apply` and spread read the same way.
///
/// `apply` reads `length` and each index; spread calls
/// `Array.prototype[Symbol.iterator]`, which reads the same for an Array (holes
/// included) and for the `arguments` object while the builtins are unpatched
/// (`stable_builtins`). So a proven argument list is:
///
/// - an array literal, `new Array(…)`, `Array(…)`, `Array.from(…)`,
///   `Array.of(…)`, or a `concat`, `filter`,
///   `flat`, `flatMap`, `map` or `slice` call on a proven Array (producer
///   `babel-core@6.26.3` es2015 emits `[a].concat(rest)`);
/// - the `arguments` object, unless the module assigns to it (producer
///   `babel-core@6.26.3` es2015 emits `target.apply(undefined, arguments)`);
/// - an `Array.prototype.slice.call(…)` or `[].slice.call(…)` copy of that
///   `arguments` object or of a proven Array (producers `babel-core@6.26.3`
///   es2015, `@babel/preset-env@7.29` and `@swc/core@1.16.2` es5 emit
///   `Array.prototype.slice.call(arguments)` for `...arguments`; see
///   `proves_fresh_array`);
/// - a rest parameter, or a `let`/`const` initialized to an Array by
///   construction (a read in the TDZ throws either way);
/// - a `var` initialized to an Array, a declaration whose initializer is an
///   Array only if another binding holds one (`c = t.map(f)`, checked when
///   `c` becomes ready), or a recognized `arguments` copy
///   (producers `typescript@3.8.3` and `typescript@5.9.3` ES5 emit
///   `var args = []; for (…) args[_i] = arguments[_i]; f.apply(void 0, args)`),
///   read in a later statement of the same body and outside a function
///   declaration that hoisting could run earlier.
///
/// Every binding must have one declaration and no write after it, and is
/// trusted only without `with` or direct eval in the module.
#[derive(Default)]
struct ProvenArgumentLists {
    unresolved_mark: Mark,
    /// `None` until `collect` runs; nothing is proven without it.
    uses: Option<BindingUseIndex>,
    /// Rest parameters and `let`/`const` Arrays: proven wherever they resolve.
    always: HashSet<BindingId>,
    /// Activated positional candidates, keyed to the function-declaration depth
    /// of the body that declares them.
    positional: HashMap<BindingId, usize>,
    fn_decl_depth: usize,
    arguments_written: bool,
    dynamic_scope: bool,
    closure: Option<ClosureRuntime>,
}

impl ProvenArgumentLists {
    fn collect(module: &Module, unresolved_mark: Mark, dynamic_scope: bool) -> Self {
        let uses = BindingUseIndex::collect(module);
        let mut collector = AlwaysProvenCollector {
            unresolved_mark,
            uses: &uses,
            always: HashSet::default(),
            arguments_written: false,
        };
        module.visit_with(&mut collector);
        let always = collector.always;
        let arguments_written = collector.arguments_written;
        Self {
            unresolved_mark,
            uses: Some(uses),
            always,
            positional: HashMap::default(),
            fn_decl_depth: 0,
            arguments_written,
            dynamic_scope,
            closure: Some(ClosureRuntime::collect(module, unresolved_mark)),
        }
    }

    /// A Closure `arrayFromIterable` call, named or renamed: always an Array.
    fn is_closure_array_helper_call(&self, expr: &Expr) -> bool {
        self.closure.as_ref().is_some_and(|closure| {
            closure
                .array_from_iterable_source(strip_parens(expr))
                .is_some()
                || closure.is_renamed_helper_call(expr)
        })
    }

    /// The argument list carries a lowerer's spread helper: an array literal
    /// with a spread element (a recovered `_toConsumableArray`, `__spreadArray`
    /// or Closure concat), or a Closure `arrayFromIterable` call.
    fn lowered_spread_evidence(&self, expr: &Expr, dynamic_scope: bool) -> bool {
        if dynamic_scope {
            return false;
        }
        match strip_parens(expr) {
            Expr::Array(array) => array
                .elems
                .iter()
                .flatten()
                .any(|elem| elem.spread.is_some()),
            expr => self.is_closure_array_helper_call(expr),
        }
    }

    fn unwritten_single_binding(&self, binding: &BindingId) -> bool {
        self.uses.as_ref().is_some_and(|uses| {
            uses.has_single_declaration(binding) && !uses.has_direct_write(binding)
        })
    }

    fn body_candidates<T: AsStmt>(
        &self,
        items: &[T],
        copy: Option<super::arg_rest::RestArrayCopyProof>,
    ) -> Vec<PositionalCandidate> {
        if self.uses.is_none() {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let Some(Stmt::Decl(Decl::Var(var))) = item.as_stmt() else {
                continue;
            };
            for decl in &var.decls {
                let Some((binding, depends_on)) =
                    array_initialized_binding(decl, self.unresolved_mark)
                else {
                    continue;
                };
                // A self-contained `let`/`const` is already proven everywhere.
                if var.kind != VarDeclKind::Var && depends_on.is_none() {
                    continue;
                }
                if self.unwritten_single_binding(&binding) {
                    candidates.push(PositionalCandidate {
                        binding,
                        ready: index + 1,
                        depends_on,
                    });
                }
            }
        }
        if let Some(copy) = copy {
            let binding = binding_id(&copy.binding);
            if self.unwritten_single_binding(&binding) {
                candidates.push(PositionalCandidate {
                    binding,
                    ready: copy.ready_stmt,
                    depends_on: None,
                });
            }
        }
        candidates
    }

    fn activate(&mut self, candidates: &[PositionalCandidate], index: usize) {
        for candidate in candidates {
            if candidate.ready > index || self.positional.contains_key(&candidate.binding) {
                continue;
            }
            let dependency_holds = candidate.depends_on.as_ref().is_none_or(|dependency| {
                self.proves_array(
                    &Expr::Ident(dependency.clone()),
                    self.unresolved_mark,
                    self.dynamic_scope,
                )
            });
            if dependency_holds {
                self.positional
                    .insert(candidate.binding.clone(), self.fn_decl_depth);
            }
        }
    }

    fn deactivate(&mut self, candidates: &[PositionalCandidate]) {
        for candidate in candidates {
            self.positional.remove(&candidate.binding);
        }
    }

    fn proves(&self, expr: &Expr, unresolved_mark: Mark, dynamic_scope: bool) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) => {
                if dynamic_scope {
                    return false;
                }
                if is_unresolved_ident(ident, "arguments", unresolved_mark) {
                    return !self.arguments_written;
                }
                let binding = binding_id(ident);
                self.always.contains(&binding)
                    || self
                        .positional
                        .get(&binding)
                        .is_some_and(|depth| *depth == self.fn_decl_depth)
            }
            expr if !dynamic_scope && self.is_closure_array_helper_call(expr) => true,
            expr => self.proves_fresh_array(expr, unresolved_mark, dynamic_scope),
        }
    }

    /// A new plain Array that no other code holds yet: `is_array_value`, or a
    /// `slice` copy (`array_slice_call_receiver`) of the unwritten `arguments`
    /// object or of a proven Array.
    ///
    /// `Array.prototype.slice` builds its result with `ArraySpeciesCreate`,
    /// which uses the intrinsic `Array` for a receiver that is not an Array,
    /// such as `arguments`, and the receiver's `constructor` for an Array,
    /// the intrinsic one for a plain Array (`stable_builtins`, as for
    /// `ARRAY_RESULT_METHODS`). Any other receiver may be an Array subclass
    /// whose species builds something else, so it stays unproven.
    fn proves_fresh_array(&self, expr: &Expr, unresolved_mark: Mark, dynamic_scope: bool) -> bool {
        if is_array_value(expr, unresolved_mark, &|receiver| {
            self.proves_array(receiver, unresolved_mark, dynamic_scope)
        }) {
            return true;
        }
        if dynamic_scope {
            return false;
        }
        let Some(receiver) = array_slice_call_receiver(expr, unresolved_mark) else {
            return false;
        };
        match strip_parens(receiver) {
            Expr::Ident(ident) if is_unresolved_ident(ident, "arguments", unresolved_mark) => {
                !self.arguments_written
            }
            receiver => self.proves_array(receiver, unresolved_mark, dynamic_scope),
        }
    }

    /// Like `proves`, but excludes the `arguments` object, which has no
    /// `concat` method.
    fn proves_array(&self, expr: &Expr, unresolved_mark: Mark, dynamic_scope: bool) -> bool {
        match strip_parens(expr) {
            Expr::Ident(ident) if ident.sym == "arguments" => false,
            expr => self.proves(expr, unresolved_mark, dynamic_scope),
        }
    }
}

/// An Array by construction: an array literal, `new Array(…)`, `Array(…)`,
/// `Array.from(…)` or `Array.of(…)` on the global, or an
/// `ARRAY_RESULT_METHODS` call on a receiver `is_array` accepts.
fn is_array_value(expr: &Expr, unresolved_mark: Mark, is_array: &dyn Fn(&Expr) -> bool) -> bool {
    match strip_parens(expr) {
        Expr::Array(_) => true,
        Expr::New(new) => {
            matches!(new.callee.as_ref(), Expr::Ident(id) if is_unresolved_ident(id, "Array", unresolved_mark))
        }
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return false;
            };
            match strip_parens(callee) {
                Expr::Ident(id) => is_unresolved_ident(id, "Array", unresolved_mark),
                Expr::Member(MemberExpr {
                    obj,
                    prop: MemberProp::Ident(prop),
                    ..
                }) => {
                    if ARRAY_RESULT_METHODS.contains(&prop.sym.as_ref()) {
                        return is_array(obj);
                    }
                    // `Array.from(…)` / `Array.of(…)` construct with `this`,
                    // the global `Array` here.
                    matches!(prop.sym.as_ref(), "from" | "of")
                        && matches!(strip_parens(obj), Expr::Ident(id) if is_unresolved_ident(id, "Array", unresolved_mark))
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// `X` of `Array.prototype.slice.call(X, …)` or `[…].slice.call(X, …)`, with
/// `slice` read from the global `Array` or an array literal. The remaining
/// arguments (`start`, `end`) are not inspected: the call stays in the
/// output, so they run the same either way.
fn array_slice_call_receiver(expr: &Expr, unresolved_mark: Mark) -> Option<&Expr> {
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    let Callee::Expr(callee) = &call.callee else {
        return None;
    };
    let Expr::Member(MemberExpr {
        obj: slice,
        prop: MemberProp::Ident(call_prop),
        ..
    }) = strip_parens(callee)
    else {
        return None;
    };
    let Expr::Member(MemberExpr {
        obj: owner,
        prop: MemberProp::Ident(slice_prop),
        ..
    }) = strip_parens(slice)
    else {
        return None;
    };
    if call_prop.sym != "call" || slice_prop.sym != "slice" {
        return None;
    }
    let owner_is_array_prototype = match strip_parens(owner) {
        Expr::Array(_) => true,
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) => {
            prop.sym == "prototype"
                && matches!(strip_parens(obj), Expr::Ident(id) if is_unresolved_ident(id, "Array", unresolved_mark))
        }
        _ => false,
    };
    if !owner_is_array_prototype {
        return None;
    }
    match call.args.first() {
        Some(receiver) if receiver.spread.is_none() => Some(&receiver.expr),
        _ => None,
    }
}

/// The `arguments` of `Array.prototype.slice.call(arguments)` or
/// `….slice.call(arguments, 0)`: a whole copy of the `arguments` object.
fn arguments_copy_source(expr: &Expr, unresolved_mark: Mark) -> Option<&Ident> {
    let Expr::Ident(arguments) = strip_parens(array_slice_call_receiver(expr, unresolved_mark)?)
    else {
        return None;
    };
    if !is_unresolved_ident(arguments, "arguments", unresolved_mark) {
        return None;
    }
    let Expr::Call(call) = strip_parens(expr) else {
        return None;
    };
    match call.args.as_slice() {
        [_] => Some(arguments),
        [_, start]
            if start.spread.is_none()
                && matches!(strip_parens(&start.expr), Expr::Lit(Lit::Num(num)) if num.value == 0.0) =>
        {
            Some(arguments)
        }
        _ => None,
    }
}

/// The binding of `x = <Array>` before any write check, with the binding the
/// initializer's Array proof depends on (`t` in `x = t.map(f)`), if any.
fn array_initialized_binding(
    decl: &VarDeclarator,
    unresolved_mark: Mark,
) -> Option<(BindingId, Option<Ident>)> {
    let Pat::Ident(name) = &decl.name else {
        return None;
    };
    let depends_on = array_dependency(decl.init.as_deref()?, unresolved_mark)?;
    Some((binding_id(&name.id), depends_on))
}

/// `Some(None)` for an Array by construction, `Some(Some(t))` for an Array
/// whenever `t` holds one, `None` otherwise.
fn array_dependency(expr: &Expr, unresolved_mark: Mark) -> Option<Option<Ident>> {
    match strip_parens(expr) {
        Expr::Ident(ident) if !is_unresolved_ident(ident, "arguments", unresolved_mark) => {
            Some(Some(ident.clone()))
        }
        Expr::Call(call) => {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(MemberExpr {
                    obj,
                    prop: MemberProp::Ident(prop),
                    ..
                }) = strip_parens(callee)
                {
                    if ARRAY_RESULT_METHODS.contains(&prop.sym.as_ref()) {
                        return array_dependency(obj, unresolved_mark);
                    }
                }
            }
            is_array_value(expr, unresolved_mark, &|_| false).then_some(None)
        }
        expr => is_array_value(expr, unresolved_mark, &|_| false).then_some(None),
    }
}

trait AsStmt {
    fn as_stmt(&self) -> Option<&Stmt>;
}

impl AsStmt for Stmt {
    fn as_stmt(&self) -> Option<&Stmt> {
        Some(self)
    }
}

impl AsStmt for ModuleItem {
    fn as_stmt(&self) -> Option<&Stmt> {
        match self {
            ModuleItem::Stmt(stmt) => Some(stmt),
            ModuleItem::ModuleDecl(_) => None,
        }
    }
}

struct AlwaysProvenCollector<'a> {
    unresolved_mark: Mark,
    uses: &'a BindingUseIndex,
    always: HashSet<BindingId>,
    arguments_written: bool,
}

impl AlwaysProvenCollector<'_> {
    fn add_if_unwritten(&mut self, binding: BindingId) {
        if self.uses.has_single_declaration(&binding) && !self.uses.has_direct_write(&binding) {
            self.always.insert(binding);
        }
    }

    fn add_rest_params<'p>(&mut self, params: impl Iterator<Item = &'p Pat>) {
        for param in params {
            if let Pat::Rest(rest) = param {
                if let Pat::Ident(binding) = rest.arg.as_ref() {
                    self.add_if_unwritten(binding_id(&binding.id));
                }
            }
        }
    }
}

impl Visit for AlwaysProvenCollector<'_> {
    fn visit_function(&mut self, function: &Function) {
        self.add_rest_params(function.params.iter().map(|param| &param.pat));
        function.visit_children_with(self);
    }

    fn visit_constructor(&mut self, constructor: &Constructor) {
        self.add_rest_params(constructor.params.iter().filter_map(|param| match param {
            ParamOrTsParamProp::Param(param) => Some(&param.pat),
            ParamOrTsParamProp::TsParamProp(_) => None,
        }));
        constructor.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        self.add_rest_params(arrow.params.iter());
        arrow.visit_children_with(self);
    }

    // A `let`/`const` read before its declaration throws in both forms, so
    // its proof does not depend on position.
    fn visit_var_decl(&mut self, var: &swc_core::ecma::ast::VarDecl) {
        if var.kind != VarDeclKind::Var {
            for decl in &var.decls {
                if let Some((binding, None)) = array_initialized_binding(decl, self.unresolved_mark)
                {
                    self.add_if_unwritten(binding);
                }
            }
        }
        var.visit_children_with(self);
    }

    // `arguments = …`, `arguments++`, or `arguments` as a destructuring or
    // loop-head target. A declared `arguments` binding is not unresolved and
    // is never proven, so declarations need no exclusion here.
    fn visit_binding_ident(&mut self, binding: &BindingIdent) {
        if is_unresolved_ident(&binding.id, "arguments", self.unresolved_mark) {
            self.arguments_written = true;
        }
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if matches!(strip_parens(&update.arg), Expr::Ident(id) if is_unresolved_ident(id, "arguments", self.unresolved_mark))
        {
            self.arguments_written = true;
        }
        update.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Paren(paren)) = &assign.left {
            if matches!(strip_parens(&paren.expr), Expr::Ident(id) if is_unresolved_ident(id, "arguments", self.unresolved_mark))
            {
                self.arguments_written = true;
            }
        }
        assign.visit_children_with(self);
    }
}
