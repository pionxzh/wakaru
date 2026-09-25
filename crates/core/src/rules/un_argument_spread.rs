use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignExpr, AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprOrSpread, ExprStmt, Ident, Lit,
    MemberExpr, MemberProp, Module, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use super::decl_utils::{
    can_remove_prior_uninitialized_decls, remove_prior_uninitialized_decls, same_ident,
    UninitializedDeclKind,
};
use super::eval_utils::has_dynamic_scope_construct;
use super::expr_utils::{exprs_structurally_equal, is_unresolved_undefined};
use super::RewriteLevel;

use crate::analysis::binding_uses::{BindingId, BindingUseIndex};
use crate::collections::{HashMap, HashSet};
use crate::utils::paren::strip_parens;

pub struct UnArgumentSpread {
    unresolved_mark: Mark,
    level: RewriteLevel,
    /// Memoized-apply temps whose every use is the write and the `thisArg`
    /// read of a pattern this rule matches. Only these lose their
    /// assignment; any other temp keeps `(t = expr)` on the callee object,
    /// where it still runs before the arguments.
    isolated_temps: HashSet<BindingId>,
}

impl UnArgumentSpread {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
            isolated_temps: HashSet::default(),
        }
    }

    fn is_isolated_temp(&self, ident: &Ident) -> bool {
        self.isolated_temps
            .contains(&(ident.sym.clone(), ident.ctxt))
    }
}

impl Default for UnArgumentSpread {
    fn default() -> Self {
        Self::new(Mark::new(), RewriteLevel::Standard)
    }
}

impl VisitMut for UnArgumentSpread {
    fn visit_mut_module(&mut self, module: &mut Module) {
        if self.level >= RewriteLevel::Standard {
            self.isolated_temps = collect_isolated_temps(module);
        }
        module.visit_mut_children_with(self);
    }

    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        let mut old = std::mem::take(stmts);
        let mut index = 0;
        while index < old.len() {
            if index + 1 < old.len() {
                if let Some(rewrite) =
                    try_convert_split_memoized_apply(&old[index], &old[index + 1], |temp| {
                        self.is_isolated_temp(temp)
                    })
                {
                    if can_remove_prior_uninitialized_decls(
                        stmts,
                        &rewrite.removable_bindings,
                        UninitializedDeclKind::Any,
                    ) {
                        let end = stmts.len();
                        remove_prior_uninitialized_decls(
                            stmts,
                            end,
                            &rewrite.removable_bindings,
                            UninitializedDeclKind::Any,
                        );
                        stmts.push(rewrite.stmt);
                        index += 2;
                        continue;
                    }
                }
            }

            stmts.push(std::mem::replace(
                &mut old[index],
                Stmt::Empty(swc_core::ecma::ast::EmptyStmt { span: DUMMY_SP }),
            ));
            index += 1;
        }
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        let taken = match expr {
            Expr::Call(_) => {
                let placeholder = Expr::Lit(Lit::Num(swc_core::ecma::ast::Number {
                    span: DUMMY_SP,
                    value: 0.0,
                    raw: None,
                }));
                std::mem::replace(expr, placeholder)
            }
            _ => return,
        };

        let Expr::Call(call) = taken else {
            *expr = taken;
            return;
        };

        match try_convert_apply(call, self.unresolved_mark, |temp| {
            self.is_isolated_temp(temp)
        }) {
            Ok(new_expr) => *expr = new_expr,
            Err(original_call) => *expr = Expr::Call(original_call),
        }
    }
}

fn try_convert_apply(
    call: CallExpr,
    unresolved_mark: Mark,
    is_isolated_temp: impl Fn(&Ident) -> bool,
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
            let receiver = memoized_receiver(assign, &is_isolated_temp);
            return Ok(make_spread_call_with_member_receiver(call, receiver));
        }
        // obj.fn.apply(null/undefined, ...) — Babel spread artifact for standalone
        // function calls on module namespaces (e.g. `r.applyMiddleware.apply(void 0, d)`).
        // Not converted here because it changes `this` from undefined to obj.
        // The proper fix is namespace import decomposition (r.fn → fn), after which
        // Pattern 1 (simple ident) handles it.
        return Err(call);
    }

    // Pattern 1: callee obj is not a member expression, first arg must be null/undefined
    if matches!(first_arg, Expr::Lit(Lit::Null(_)))
        || is_unresolved_undefined(first_arg, unresolved_mark)
    {
        return Ok(make_spread_call(call));
    }

    Err(call)
}

/// A matched `method = <receiver>.prop; method.apply(thisArg, args)` pair.
struct SplitMemoizedApply<'a> {
    method_temp: Ident,
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
        method_temp,
        member,
        apply_call,
        receiver,
    })
}

fn try_convert_split_memoized_apply(
    method_stmt: &Stmt,
    apply_stmt: &Stmt,
    is_isolated_temp: impl Fn(&Ident) -> bool,
) -> Option<SplitMemoizedApplyRewrite> {
    let matched = match_split_memoized_apply(method_stmt, apply_stmt)?;

    // The method statement is deleted, so the method temp must have no use
    // outside the pair: not in the arguments, not inside the kept member,
    // and nowhere else in the module.
    if !is_isolated_temp(&matched.method_temp) {
        return None;
    }
    let mut removable_bindings = vec![matched.method_temp.clone()];

    let receiver = match matched.receiver {
        SplitReceiver::Direct => matched.member.obj.clone(),
        SplitReceiver::Memoized(assign) => {
            let receiver = memoized_receiver(assign, &is_isolated_temp);
            if !matches!(receiver.as_ref(), Expr::Assign(_)) {
                removable_bindings.extend(memoized_assign_target(assign).cloned());
            }
            receiver
        }
    };

    let apply_call = matched.apply_call;
    let mut args = args_from_apply_arg(apply_call.args[1].expr.clone());
    let callee = Expr::Member(MemberExpr {
        span: matched.member.span,
        obj: receiver,
        prop: matched.member.prop.clone(),
    });

    Some(SplitMemoizedApplyRewrite {
        stmt: Stmt::Expr(ExprStmt {
            span: apply_call.span,
            expr: Box::new(Expr::Call(CallExpr {
                span: apply_call.span,
                ctxt: apply_call.ctxt,
                callee: Callee::Expr(Box::new(callee)),
                args: std::mem::take(&mut args),
                type_args: apply_call.type_args.clone(),
            })),
        }),
        removable_bindings,
    })
}

struct SplitMemoizedApplyRewrite {
    stmt: Stmt,
    removable_bindings: Vec<Ident>,
}

/// Collect the memoized-apply temps whose assignment can be dropped: every
/// use is one this rule's patterns consume (the write and the `thisArg`
/// read, or the method temp's write and `.apply` read), and the binding's
/// only declaration is an uninitialized `var`/`let` declarator the patterns
/// may assign (a `let` in its TDZ there would throw, which dropping the
/// write would hide). A parameter is excluded because sloppy-mode
/// `arguments` aliases it.
fn collect_isolated_temps(module: &Module) -> HashSet<BindingId> {
    let mut counter = PatternUseCounter::default();
    module.visit_with(&mut counter);
    if counter.uses.is_empty() || has_dynamic_scope_construct(module) {
        return HashSet::default();
    }

    let index = BindingUseIndex::collect(module);
    let assignable = index.assignable_uninitialized_bindings();
    counter
        .uses
        .into_iter()
        .filter(|(binding, pattern_uses)| {
            assignable.contains(binding)
                && index.has_single_declaration(binding)
                && index.use_count(binding) == *pattern_uses
        })
        .map(|(binding, _)| binding)
        .collect()
}

/// Counts the temp uses each matched pattern would consume. Each counted
/// write is read only by its own pattern, so a binding whose total use
/// count equals this count has no reader outside the patterns.
#[derive(Default)]
struct PatternUseCounter {
    uses: HashMap<BindingId, usize>,
}

impl PatternUseCounter {
    fn add(&mut self, ident: &Ident, count: usize) {
        *self
            .uses
            .entry((ident.sym.clone(), ident.ctxt))
            .or_default() += count;
    }
}

impl Visit for PatternUseCounter {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Some(assign) = expr_memoized_apply_assign(call) {
            if let Some(target) = memoized_assign_target(assign) {
                self.add(target, 2);
            }
        }
        call.visit_children_with(self);
    }

    fn visit_stmts(&mut self, stmts: &[Stmt]) {
        for pair in stmts.windows(2) {
            let Some(matched) = match_split_memoized_apply(&pair[0], &pair[1]) else {
                continue;
            };
            self.add(&matched.method_temp, 2);
            if let SplitReceiver::Memoized(assign) = matched.receiver {
                if let Some(target) = memoized_assign_target(assign) {
                    self.add(target, 2);
                }
            }
        }
        stmts.visit_children_with(self);
    }
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
/// preserves the write for readers in the arguments or later code; it is
/// dropped only for a temp nothing else reads.
fn memoized_receiver(assign: &AssignExpr, is_isolated_temp: impl Fn(&Ident) -> bool) -> Box<Expr> {
    if memoized_assign_target(assign).is_some_and(is_isolated_temp) {
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
