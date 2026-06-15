use swc_core::common::{Mark, DUMMY_SP};
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, CallExpr, Callee, Expr, ExprOrSpread, ExprStmt, Ident, Lit, MemberExpr,
    MemberProp, SimpleAssignTarget, Stmt,
};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::decl_utils::{
    can_remove_prior_uninitialized_decls, ident_is_used_in_stmts_excluding_bindings,
    remove_prior_uninitialized_decls, same_ident, UninitializedDeclKind,
};
use super::expr_utils::{exprs_structurally_equal, is_unresolved_undefined};
use super::RewriteLevel;

use crate::utils::paren::strip_parens;

pub struct UnArgumentSpread {
    unresolved_mark: Mark,
    level: RewriteLevel,
}

impl UnArgumentSpread {
    pub fn new(unresolved_mark: Mark, level: RewriteLevel) -> Self {
        Self {
            unresolved_mark,
            level,
        }
    }
}

impl Default for UnArgumentSpread {
    fn default() -> Self {
        Self::new(Mark::new(), RewriteLevel::Standard)
    }
}

impl VisitMut for UnArgumentSpread {
    fn visit_mut_stmts(&mut self, stmts: &mut Vec<Stmt>) {
        stmts.visit_mut_children_with(self);

        if self.level < RewriteLevel::Standard {
            return;
        }

        let old = std::mem::take(stmts);
        let mut index = 0;
        while index < old.len() {
            if index + 1 < old.len() {
                if let Some(rewrite) = try_convert_split_memoized_apply(
                    &old[index],
                    &old[index + 1],
                    &old[index + 2..],
                ) {
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

            stmts.push(old[index].clone());
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

        match try_convert_apply(call, self.unresolved_mark) {
            Ok(new_expr) => *expr = new_expr,
            Err(original_call) => *expr = Expr::Call(original_call),
        }
    }
}

fn try_convert_apply(call: CallExpr, unresolved_mark: Mark) -> Result<Expr, CallExpr> {
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
        if exprs_structurally_equal(first_arg, &callee_member_obj.obj) {
            return Ok(make_spread_call(call));
        }
        if let Some(receiver) = memoized_receiver_source(&callee_member_obj.obj, first_arg) {
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

fn try_convert_split_memoized_apply(
    method_stmt: &Stmt,
    apply_stmt: &Stmt,
    rest: &[Stmt],
) -> Option<SplitMemoizedApplyRewrite> {
    let (method_temp, memoized_member) = memoized_method_assignment(method_stmt)?;
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
    let mut removable_bindings = vec![method_temp.clone()];
    let receiver = if exprs_structurally_equal(first_arg, &memoized_member.obj) {
        memoized_member.obj.clone()
    } else {
        let receiver_temp = ident_expr(first_arg)?;
        removable_bindings.push(receiver_temp.clone());
        memoized_receiver_source(&memoized_member.obj, first_arg)?
    };

    if removable_bindings
        .iter()
        .any(|binding| ident_is_used_in_stmts_excluding_bindings(binding, rest))
    {
        return None;
    }

    let mut args = args_from_apply_arg(apply_call.args[1].expr.clone());
    let callee = Expr::Member(MemberExpr {
        span: memoized_member.span,
        obj: receiver,
        prop: memoized_member.prop.clone(),
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

fn ident_expr(expr: &Expr) -> Option<&Ident> {
    match expr {
        Expr::Ident(ident) => Some(ident),
        _ => None,
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

fn memoized_receiver_source(receiver_expr: &Expr, first_arg: &Expr) -> Option<Box<Expr>> {
    let receiver_expr = strip_parens(receiver_expr);
    let Expr::Assign(assign) = receiver_expr else {
        return None;
    };
    if assign.op != AssignOp::Assign {
        return None;
    }
    let AssignTarget::Simple(SimpleAssignTarget::Ident(target)) = &assign.left else {
        return None;
    };
    if !matches!(first_arg, Expr::Ident(id) if id.sym == target.id.sym && id.ctxt == target.id.ctxt)
    {
        return None;
    }
    Some(assign.right.clone())
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
