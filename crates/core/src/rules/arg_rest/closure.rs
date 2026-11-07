//! Closure Compiler's rest-parameter lowering.
//!
//! Closure lowers `function f(a, ...xs)` to a call of its runtime helper:
//!
//! ```text
//! $jscomp.getRestArguments = function() {
//!   for (var a = Number(this), b = [], c = a; c < arguments.length; c++)
//!     b[c - a] = arguments[c];
//!   return b;
//! };
//! function f(a) { var xs = $jscomp.getRestArguments.apply(1, arguments); ... }
//! ```
//!
//! The receiver of `.apply` is the start index, the number of fixed
//! parameters. The helper returns a fresh Array of `arguments[K..]`, which is
//! the value a rest parameter holds. ADVANCED output renames the helper and
//! may inline the call into an expression instead of a declaration.

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinaryOp, CallExpr, Callee, Constructor, Decl, DoWhileStmt, Expr,
    ForInStmt, ForOfStmt, ForStmt, Function, FunctionBody, Ident, Lit, MemberProp, Module, Pat,
    ReturnStmt, SimpleAssignTarget, Stmt, UpdateOp, VarDeclOrExpr, VarDeclarator, WhileStmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::analysis::binding_uses::BindingUseIndex;
use crate::analysis::{binding_id, BindingId};
use crate::collections::HashSet;
use crate::rules::eval_utils::has_dynamic_scope_construct;
use crate::rules::helper_matcher::{count_binding_refs, static_member_prop_name};
use crate::rules::un_array_concat_spread::{collect_function_bindings, ClosureRuntime};
use crate::utils::paren::strip_parens;

use super::{prepare_rest_ident, IdentNameCollector};

/// The rest helpers one module can call: `$jscomp.getRestArguments` trusted
/// by name like `$jscomp.makeIterator` (UnForOf) and
/// `$jscomp.arrayFromIterable` (UnArrayConcatSpreadRest), and renamed
/// ADVANCED helpers proven by their whole body.
pub(super) struct ClosureRestRuntime {
    runtime: ClosureRuntime,
    rest_helpers: HashSet<BindingId>,
    unresolved_mark: Mark,
}

impl ClosureRestRuntime {
    /// `None` when no `X.apply(<number>, arguments)` call exists, and under a
    /// `with` statement or direct `eval`: the rewrite introduces and removes
    /// bindings, and a direct eval can read `arguments` unseen.
    pub(super) fn collect(module: &Module, unresolved_mark: Mark) -> Option<Self> {
        if !module_has_apply_arguments_call(module) || has_dynamic_scope_construct(module) {
            return None;
        }
        let candidates = collect_function_bindings(module, |function| {
            is_get_rest_arguments_body(function, unresolved_mark)
        });
        let rest_helpers = if candidates.is_empty() {
            candidates
        } else {
            let uses = BindingUseIndex::collect(module);
            candidates
                .into_iter()
                .filter(|binding| {
                    uses.has_single_declaration(binding) && !uses.has_direct_write(binding)
                })
                .collect()
        };
        Some(Self {
            runtime: ClosureRuntime::collect(module, unresolved_mark),
            rest_helpers,
            unresolved_mark,
        })
    }

    /// `K` in `H.apply(K, arguments)`, where `H` is a rest helper and
    /// `arguments` is the implicit binding.
    fn rest_call_start(&self, call: &CallExpr) -> Option<usize> {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        let Expr::Member(member) = strip_parens(callee) else {
            return None;
        };
        if static_member_prop_name(&member.prop) != Some("apply")
            || !self.is_rest_helper(&member.obj)
        {
            return None;
        }
        let [start, args] = call.args.as_slice() else {
            return None;
        };
        if start.spread.is_some()
            || args.spread.is_some()
            || !self.is_implicit_arguments(&args.expr)
        {
            return None;
        }
        let Expr::Lit(Lit::Num(number)) = strip_parens(&start.expr) else {
            return None;
        };
        (number.value >= 0.0 && number.value.fract() == 0.0).then_some(number.value as usize)
    }

    fn is_rest_helper(&self, expr: &Expr) -> bool {
        match strip_parens(expr) {
            Expr::Ident(helper) => self.rest_helpers.contains(&binding_id(helper)),
            expr => self.runtime.is_jscomp_member(expr, "getRestArguments"),
        }
    }

    fn is_implicit_arguments(&self, expr: &Expr) -> bool {
        matches!(strip_parens(expr), Expr::Ident(id)
            if id.sym == "arguments" && id.ctxt.outer() == self.unresolved_mark)
    }
}

/// Replace the function's one rest-helper call with a rest parameter and
/// return that parameter's binding. `var xs = H.apply(K, arguments)` as a
/// top-level body statement reuses `xs`; a call anywhere else becomes a
/// fresh `args`. Bails unless the call is the only mention of `arguments`
/// (nested arrows included), `K` is the number of fixed parameters, and the
/// call runs at most once per invocation: outside loops and nested arrows, so
/// one rest Array taken at entry stands in for the one copy. Indices from `K`
/// up are not mapped to parameters, so the copy reads the same values at
/// entry as where the call stood.
pub(super) fn recover_closure_rest<P>(
    body: &mut FunctionBody,
    params: &P,
    param_bindings: &HashSet<BindingId>,
    fixed_param_count: usize,
    runtime: &ClosureRestRuntime,
) -> Option<Ident>
where
    P: VisitWith<IdentNameCollector>,
{
    let mut finder = RestCallFinder {
        runtime,
        repeat_depth: 0,
        calls: Vec::new(),
        other_arguments: false,
    };
    body.visit_with(&mut finder);
    let [(start, repeats)] = finder.calls.as_slice() else {
        return None;
    };
    if finder.other_arguments || *repeats || *start != fixed_param_count {
        return None;
    }

    let rest = match find_rest_declaration(body, runtime) {
        Some((stmt, decl)) => {
            let Pat::Ident(binding) = &var_declarators(&body.stmts[stmt])[decl].name else {
                return None;
            };
            let binding = binding.id.clone();
            if !rest_declaration_is_movable(body, stmt, decl, &binding, param_bindings) {
                return None;
            }
            let rest = prepare_rest_ident(body, params, Some(&binding));
            remove_declarator(body, stmt, decl);
            rest
        }
        None => prepare_rest_ident(body, params, None),
    };

    body.visit_mut_with(&mut RestCallReplacer {
        runtime,
        rest: &rest,
    });
    let rest_binding = binding_id(&rest);
    if !BindingUseIndex::collect_stmts(&body.stmts).has_direct_write(&rest_binding) {
        body.visit_mut_with(&mut ArrayFromIterableUnwrapper {
            runtime: &runtime.runtime,
            rest: rest_binding,
        });
    }
    Some(rest)
}

/// `(statement, declarator)` of a top-level `var xs = H.apply(K, arguments)`.
fn find_rest_declaration(
    body: &FunctionBody,
    runtime: &ClosureRestRuntime,
) -> Option<(usize, usize)> {
    body.stmts.iter().enumerate().find_map(|(index, stmt)| {
        var_declarators(stmt)
            .iter()
            .position(|decl| {
                matches!(decl.init.as_deref().map(strip_parens), Some(Expr::Call(call))
                    if runtime.rest_call_start(call).is_some())
            })
            .map(|decl| (index, decl))
    })
}

fn var_declarators(stmt: &Stmt) -> &[VarDeclarator] {
    match stmt {
        Stmt::Decl(Decl::Var(var)) => &var.decls,
        _ => &[],
    }
}

/// The binding becomes a parameter, initialized at entry instead of at its
/// declaration. That is unobservable only when it has no other declaration,
/// is not itself a parameter, and nothing can read it earlier: no reference
/// in the code before the declarator, and, when such code exists, none in a
/// hoisted function declaration that code could call.
fn rest_declaration_is_movable(
    body: &FunctionBody,
    stmt: usize,
    decl: usize,
    binding: &Ident,
    param_bindings: &HashSet<BindingId>,
) -> bool {
    let key = binding_id(binding);
    if param_bindings.contains(&key)
        || !BindingUseIndex::collect_stmts(&body.stmts).has_single_declaration(&key)
    {
        return false;
    }
    let earlier_decls = &var_declarators(&body.stmts[stmt])[..decl];
    if body.stmts[..stmt]
        .iter()
        .any(|stmt| count_binding_refs(stmt, &key) > 0)
        || earlier_decls
            .iter()
            .any(|decl| count_binding_refs(decl, &key) > 0)
    {
        return false;
    }
    if stmt == 0 && decl == 0 {
        return true;
    }
    let mut finder = FnDeclRefFinder {
        key: &key,
        in_fn_decl: false,
        found: false,
    };
    body.visit_with(&mut finder);
    !finder.found
}

fn remove_declarator(body: &mut FunctionBody, stmt: usize, decl: usize) {
    let Stmt::Decl(Decl::Var(var)) = &mut body.stmts[stmt] else {
        return;
    };
    var.decls.remove(decl);
    if var.decls.is_empty() {
        body.stmts.remove(stmt);
    }
}

/// Collects rest-helper calls with whether they can run more than once per
/// invocation, and any other mention of `arguments`. Nested functions and
/// constructors have their own `arguments`; arrows share it but can run
/// repeatedly.
struct RestCallFinder<'a> {
    runtime: &'a ClosureRestRuntime,
    repeat_depth: usize,
    calls: Vec<(usize, bool)>,
    other_arguments: bool,
}

impl RestCallFinder<'_> {
    fn repeated<N: VisitWith<Self> + ?Sized>(&mut self, node: &N) {
        self.repeat_depth += 1;
        node.visit_with(self);
        self.repeat_depth -= 1;
    }
}

impl Visit for RestCallFinder<'_> {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(start) = self.runtime.rest_call_start(call) {
            self.calls.push((start, self.repeat_depth > 0));
            return;
        }
        call.visit_children_with(self);
    }

    fn visit_ident(&mut self, id: &Ident) {
        if id.sym == "arguments" {
            self.other_arguments = true;
        }
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_core::ecma::ast::ArrowExpr) {
        self.repeat_depth += 1;
        arrow.visit_children_with(self);
        self.repeat_depth -= 1;
    }

    fn visit_for_stmt(&mut self, stmt: &ForStmt) {
        stmt.init.visit_with(self);
        self.repeated(&stmt.test);
        self.repeated(&stmt.update);
        self.repeated(&*stmt.body);
    }

    fn visit_for_in_stmt(&mut self, stmt: &ForInStmt) {
        stmt.right.visit_with(self);
        self.repeated(&stmt.left);
        self.repeated(&*stmt.body);
    }

    fn visit_for_of_stmt(&mut self, stmt: &ForOfStmt) {
        stmt.right.visit_with(self);
        self.repeated(&stmt.left);
        self.repeated(&*stmt.body);
    }

    fn visit_while_stmt(&mut self, stmt: &WhileStmt) {
        self.repeated(&*stmt.test);
        self.repeated(&*stmt.body);
    }

    fn visit_do_while_stmt(&mut self, stmt: &DoWhileStmt) {
        self.repeated(&*stmt.body);
        self.repeated(&*stmt.test);
    }

    fn visit_function(&mut self, _: &Function) {}
    fn visit_constructor(&mut self, _: &Constructor) {}
}

/// Whether a function declaration anywhere in the body references `key`.
struct FnDeclRefFinder<'a> {
    key: &'a BindingId,
    in_fn_decl: bool,
    found: bool,
}

impl Visit for FnDeclRefFinder<'_> {
    fn visit_fn_decl(&mut self, decl: &swc_core::ecma::ast::FnDecl) {
        let outer = self.in_fn_decl;
        self.in_fn_decl = true;
        decl.visit_children_with(self);
        self.in_fn_decl = outer;
    }

    fn visit_ident(&mut self, id: &Ident) {
        if self.in_fn_decl && binding_id(id) == *self.key {
            self.found = true;
        }
    }
}

struct RestCallReplacer<'a> {
    runtime: &'a ClosureRestRuntime,
    rest: &'a Ident,
}

impl VisitMut for RestCallReplacer<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if self.runtime.rest_call_start(call).is_some() {
                *expr = Expr::Ident(self.rest.clone());
                return;
            }
        }
        expr.visit_mut_children_with(self);
    }

    fn visit_mut_function(&mut self, _: &mut Function) {}
    fn visit_mut_constructor(&mut self, _: &mut Constructor) {}
}

/// `arrayFromIterable(xs)` → `xs` for the rest binding: it is an Array while
/// never written, and every recognized helper returns an Array unchanged.
struct ArrayFromIterableUnwrapper<'a> {
    runtime: &'a ClosureRuntime,
    rest: BindingId,
}

impl VisitMut for ArrayFromIterableUnwrapper<'_> {
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);
        let Some(Expr::Ident(source)) = self
            .runtime
            .array_from_iterable_argument(expr)
            .map(strip_parens)
        else {
            return;
        };
        if binding_id(source) == self.rest {
            *expr = Expr::Ident(source.clone());
        }
    }
}

/// Cheap pre-check before collecting the runtime: any `X.apply(<number>,
/// arguments)` call.
fn module_has_apply_arguments_call(module: &Module) -> bool {
    struct Finder(bool);

    impl Visit for Finder {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if self.0 {
                return;
            }
            if let (Callee::Expr(callee), [start, args]) = (&call.callee, call.args.as_slice()) {
                if matches!(strip_parens(callee), Expr::Member(member)
                        if static_member_prop_name(&member.prop) == Some("apply"))
                    && matches!(strip_parens(&start.expr), Expr::Lit(Lit::Num(_)))
                    && matches!(strip_parens(&args.expr), Expr::Ident(id) if id.sym == "arguments")
                {
                    self.0 = true;
                    return;
                }
            }
            call.visit_children_with(self);
        }
    }

    let mut finder = Finder(false);
    module.visit_with(&mut finder);
    finder.0
}

/// The whole body of Closure's `getRestArguments`, in its emitted form or
/// after earlier rules split the loop head's declarations into statements:
///
/// ```text
/// function () {
///   for (var a = Number(this), b = [], c = a; c < arguments.length; c++)
///     b[c - a] = arguments[c];
///   return b;
/// }
/// ```
///
/// Matching every statement is the proof: the body returns a fresh Array of
/// `arguments[Number(this)..]`. A castArray-like or slicing lookalike has a
/// different body.
fn is_get_rest_arguments_body(function: &Function, unresolved_mark: Mark) -> bool {
    if function.is_async || function.is_generator || !function.params.is_empty() {
        return false;
    }
    let Some(body) = &function.body else {
        return false;
    };
    let Some((Stmt::Return(ReturnStmt { arg: Some(ret), .. }), rest)) = body.stmts.split_last()
    else {
        return false;
    };
    let Some((Stmt::For(for_stmt), leading)) = rest.split_last() else {
        return false;
    };
    let Some(VarDeclOrExpr::VarDecl(loop_decl)) = &for_stmt.init else {
        return false;
    };
    let mut declarators = Vec::new();
    for stmt in leading {
        let Stmt::Decl(Decl::Var(var)) = stmt else {
            return false;
        };
        declarators.extend(var.decls.iter());
    }
    declarators.extend(loop_decl.decls.iter());
    let [start, out, index] = declarators.as_slice() else {
        return false;
    };
    let (Some((start, start_init)), Some((out, out_init)), Some((index, index_init))) = (
        declarator_parts(start),
        declarator_parts(out),
        declarator_parts(index),
    ) else {
        return false;
    };
    if start == out || start == index || out == index {
        return false;
    }

    let is_arguments = |expr: &Expr| {
        matches!(strip_parens(expr), Expr::Ident(id)
            if id.sym == "arguments" && id.ctxt.outer() == unresolved_mark)
    };
    let is = |expr: &Expr, binding: &BindingId| matches!(strip_parens(expr), Expr::Ident(id) if binding_id(id) == *binding);

    // a = Number(this)
    let Expr::Call(number_call) = strip_parens(start_init) else {
        return false;
    };
    let Callee::Expr(number) = &number_call.callee else {
        return false;
    };
    if !matches!(strip_parens(number), Expr::Ident(id)
            if id.sym == "Number" && id.ctxt.outer() == unresolved_mark)
        || !matches!(number_call.args.as_slice(), [arg]
            if arg.spread.is_none() && matches!(strip_parens(&arg.expr), Expr::This(_)))
    {
        return false;
    }
    // b = [], c = a
    if !matches!(strip_parens(out_init), Expr::Array(array) if array.elems.is_empty())
        || !is(index_init, &start)
    {
        return false;
    }
    // c < arguments.length
    let Some(Expr::Bin(test)) = for_stmt.test.as_deref().map(strip_parens) else {
        return false;
    };
    if test.op != BinaryOp::Lt
        || !is(&test.left, &index)
        || !matches!(strip_parens(&test.right), Expr::Member(member)
            if is_arguments(&member.obj)
                && matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == "length"))
    {
        return false;
    }
    // c++
    if !matches!(for_stmt.update.as_deref().map(strip_parens), Some(Expr::Update(update))
        if update.op == UpdateOp::PlusPlus && is(&update.arg, &index))
    {
        return false;
    }
    // b[c - a] = arguments[c]
    let copy = match for_stmt.body.as_ref() {
        Stmt::Block(block) => match block.stmts.as_slice() {
            [Stmt::Expr(expr)] => &expr.expr,
            _ => return false,
        },
        Stmt::Expr(expr) => &expr.expr,
        _ => return false,
    };
    let Expr::Assign(assign) = strip_parens(copy) else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return false;
    };
    let (MemberProp::Computed(target_index), Expr::Member(source)) =
        (&target.prop, strip_parens(&assign.right))
    else {
        return false;
    };
    let MemberProp::Computed(source_index) = &source.prop else {
        return false;
    };
    assign.op == AssignOp::Assign
        && is(&target.obj, &out)
        && matches!(strip_parens(&target_index.expr), Expr::Bin(sub)
            if sub.op == BinaryOp::Sub && is(&sub.left, &index) && is(&sub.right, &start))
        && is_arguments(&source.obj)
        && is(&source_index.expr, &index)
        && is(ret, &out)
}

fn declarator_parts(decl: &VarDeclarator) -> Option<(BindingId, &Expr)> {
    let Pat::Ident(name) = &decl.name else {
        return None;
    };
    Some((binding_id(&name.id), decl.init.as_deref()?))
}
