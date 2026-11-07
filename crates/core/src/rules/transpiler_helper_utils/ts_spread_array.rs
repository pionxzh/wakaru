//! Body-shape proof for tslib's `__spreadArray` (tslib 2.3 and later):
//!
//! ```js
//! function (to, from, pack) {
//!     if (pack || arguments.length === 2) for (var i = 0, l = from.length, ar; i < l; i++) {
//!         if (ar || !(i in from)) {
//!             if (!ar) ar = Array.prototype.slice.call(from, 0, i);
//!             ar[i] = from[i];
//!         }
//!     }
//!     return to.concat(ar || Array.prototype.slice.call(from));
//! }
//! ```
//!
//! A bundler that inlines the helper drops its `this && this.__spreadArray`
//! guard and its name, so the body is the only evidence. Minifiers rewrite
//! the loop body into an expression (Terser:
//! `!ar && i in from || (ar || (ar = S), ar[i] = from[i])`, esbuild:
//! `(ar || !(i in from)) && (ar || (ar = S), ar[i] = from[i])`), and
//! `UnCurlyBraces` wraps the bodies in blocks before module facts are read;
//! each of those spellings is accepted. Every identifier is matched by
//! binding, and `Array` must be the global.
//!
//! tslib 2.1 and 2.2 shipped a two-parameter `__spreadArray(to, from)` that
//! copies by index into `to`; TypeScript's call sites for it pass two
//! arguments, which the call conversion does not accept, so that body is not
//! matched here.

use swc_core::common::Mark;
use swc_core::ecma::ast::{
    AssignOp, AssignTarget, BinExpr, BinaryOp, CallExpr, Callee, Expr, ForStmt, Function, Lit,
    MemberExpr, MemberProp, Pat, SimpleAssignTarget, Stmt, UnaryOp, UpdateOp, VarDeclKind,
    VarDeclOrExpr,
};

use crate::rules::match_context::MatchContext;
use crate::utils::paren::strip_parens;

use super::is_unresolved_or_unguarded_ident;

/// Whether `function` is tslib's three-parameter `__spreadArray`.
pub(super) fn ts_spread_array_function_matches(
    function: &Function,
    unresolved_mark: Option<Mark>,
) -> bool {
    if function.is_async || function.is_generator {
        return false;
    }
    let Some(mut ctx) = MatchContext::from_params(function, &["to", "from", "pack"]) else {
        return false;
    };
    if !distinct(&ctx, &["to", "from", "pack"]) {
        return false;
    }
    let Some(body) = &function.body else {
        return false;
    };
    let [Stmt::If(guard), Stmt::Return(ret)] = body.stmts.as_slice() else {
        return false;
    };
    if guard.alt.is_some() || !is_pack_test(&guard.test, &ctx, unresolved_mark) {
        return false;
    }
    let Some(Stmt::For(for_stmt)) = single_stmt(&guard.cons) else {
        return false;
    };
    if !declare_loop_bindings(for_stmt, &mut ctx) || !distinct(&ctx, NAMES) {
        return false;
    }
    if !is_loop_head(for_stmt, &ctx) || !is_loop_body(&for_stmt.body, &ctx, unresolved_mark) {
        return false;
    }
    ret.arg.as_deref().is_some_and(|arg| {
        is_member_call(arg, &ctx, "to", "concat").is_some_and(|args| {
            matches!(args, [arg] if is_or(&arg.expr, |left| ctx.is_binding(left, "ar"), |right| {
                is_slice_call(right, &ctx, unresolved_mark, false)
            }))
        })
    })
}

const NAMES: &[&str] = &["to", "from", "pack", "ar", "i", "len"];

fn distinct(ctx: &MatchContext, names: &[&str]) -> bool {
    names.iter().enumerate().all(|(index, name)| {
        let Some(binding) = ctx.get(name) else {
            return false;
        };
        names[index + 1..]
            .iter()
            .all(|other| ctx.get(other) != Some(binding))
    })
}

/// A statement, or the one statement of a block.
fn single_stmt(stmt: &Stmt) -> Option<&Stmt> {
    match stmt {
        Stmt::Block(block) => match block.stmts.as_slice() {
            [stmt] => single_stmt(stmt),
            _ => None,
        },
        stmt => Some(stmt),
    }
}

/// `pack || arguments.length === 2`, either operand order of the comparison.
fn is_pack_test(expr: &Expr, ctx: &MatchContext, unresolved_mark: Option<Mark>) -> bool {
    is_or(
        expr,
        |left| ctx.is_binding(left, "pack"),
        |right| {
            let Expr::Bin(BinExpr {
                op: BinaryOp::EqEqEq,
                left,
                right,
                ..
            }) = right
            else {
                return false;
            };
            let is_length = |expr: &Expr| {
                matches!(strip_parens(expr), Expr::Member(MemberExpr { obj, prop: MemberProp::Ident(prop), .. })
                    if prop.sym == "length"
                        && matches!(strip_parens(obj), Expr::Ident(id) if is_unresolved_or_unguarded_ident(id, "arguments", unresolved_mark)))
            };
            let is_two = |expr: &Expr| matches!(strip_parens(expr), Expr::Lit(Lit::Num(num)) if num.value == 2.0);
            (is_length(left) && is_two(right)) || (is_two(left) && is_length(right))
        },
    )
}

/// `var i = 0, len = from.length, ar` in any order: binds the loop slots.
fn declare_loop_bindings(for_stmt: &ForStmt, ctx: &mut MatchContext) -> bool {
    let Some(VarDeclOrExpr::VarDecl(var)) = &for_stmt.init else {
        return false;
    };
    if var.kind != VarDeclKind::Var || var.decls.len() != 3 {
        return false;
    }
    for decl in &var.decls {
        let Pat::Ident(binding) = &decl.name else {
            return false;
        };
        let slot = match decl.init.as_deref().map(strip_parens) {
            None => "ar",
            Some(Expr::Lit(Lit::Num(num))) if num.value == 0.0 => "i",
            Some(Expr::Member(MemberExpr {
                obj,
                prop: MemberProp::Ident(prop),
                ..
            })) if prop.sym == "length" && ctx.is_binding(strip_parens(obj), "from") => "len",
            Some(_) => return false,
        };
        if ctx.get(slot).is_some() {
            return false;
        }
        ctx.declare(slot, binding.id.sym.clone(), binding.id.ctxt);
    }
    true
}

/// `i < len; i++`.
fn is_loop_head(for_stmt: &ForStmt, ctx: &MatchContext) -> bool {
    let test_matches = for_stmt.test.as_deref().is_some_and(|test| {
        matches!(strip_parens(test), Expr::Bin(BinExpr { op: BinaryOp::Lt, left, right, .. })
            if ctx.is_binding(strip_parens(left), "i") && ctx.is_binding(strip_parens(right), "len"))
    });
    let update_matches = for_stmt.update.as_deref().is_some_and(|update| {
        matches!(strip_parens(update), Expr::Update(update)
            if update.op == UpdateOp::PlusPlus && ctx.is_binding(strip_parens(&update.arg), "i"))
    });
    test_matches && update_matches
}

/// The copy step, in the spellings the module docs list: run the two
/// actions when `ar || !(i in from)`.
fn is_loop_body(stmt: &Stmt, ctx: &MatchContext, unresolved_mark: Option<Mark>) -> bool {
    match single_stmt(stmt) {
        Some(Stmt::If(if_stmt)) => {
            if if_stmt.alt.is_some() || !is_copy_guard(&if_stmt.test, ctx) {
                return false;
            }
            let actions: &[Stmt] = match if_stmt.cons.as_ref() {
                Stmt::Block(block) => &block.stmts,
                stmt => std::slice::from_ref(stmt),
            };
            let [init, assign] = actions else {
                return false;
            };
            is_init_stmt(init, ctx, unresolved_mark)
                && matches!(assign, Stmt::Expr(expr) if is_copy_assign(&expr.expr, ctx))
        }
        Some(Stmt::Expr(expr)) => {
            let Expr::Bin(BinExpr {
                op, left, right, ..
            }) = strip_parens(&expr.expr)
            else {
                return false;
            };
            let guard_matches = match op {
                BinaryOp::LogicalOr => is_skip_guard(left, ctx),
                BinaryOp::LogicalAnd => is_copy_guard(left, ctx),
                _ => false,
            };
            let Expr::Seq(seq) = strip_parens(right) else {
                return false;
            };
            guard_matches
                && matches!(seq.exprs.as_slice(), [init, assign]
                    if is_init_expr(init, ctx, unresolved_mark) && is_copy_assign(assign, ctx))
        }
        _ => false,
    }
}

/// `ar || !(i in from)`, or `!(!ar && i in from)`.
fn is_copy_guard(expr: &Expr, ctx: &MatchContext) -> bool {
    match strip_parens(expr) {
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => is_skip_guard(&unary.arg, ctx),
        expr => is_or(
            expr,
            |left| ctx.is_binding(left, "ar"),
            |right| matches!(right, Expr::Unary(unary) if unary.op == UnaryOp::Bang && is_index_in_from(&unary.arg, ctx)),
        ),
    }
}

/// `!ar && i in from`.
fn is_skip_guard(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Bin(BinExpr {
        op: BinaryOp::LogicalAnd,
        left,
        right,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    is_not_ar(left, ctx) && is_index_in_from(right, ctx)
}

fn is_not_ar(expr: &Expr, ctx: &MatchContext) -> bool {
    matches!(strip_parens(expr), Expr::Unary(unary)
        if unary.op == UnaryOp::Bang && ctx.is_binding(strip_parens(&unary.arg), "ar"))
}

fn is_index_in_from(expr: &Expr, ctx: &MatchContext) -> bool {
    matches!(strip_parens(expr), Expr::Bin(BinExpr { op: BinaryOp::In, left, right, .. })
        if ctx.is_binding(strip_parens(left), "i") && ctx.is_binding(strip_parens(right), "from"))
}

/// `if (!ar) ar = S;`, or the expression forms of `is_init_expr`.
fn is_init_stmt(stmt: &Stmt, ctx: &MatchContext, unresolved_mark: Option<Mark>) -> bool {
    match stmt {
        Stmt::If(if_stmt) => {
            if_stmt.alt.is_none()
                && is_not_ar(&if_stmt.test, ctx)
                && matches!(single_stmt(&if_stmt.cons), Some(Stmt::Expr(expr))
                    if is_slice_assign(&expr.expr, ctx, unresolved_mark))
        }
        Stmt::Expr(expr) => is_init_expr(&expr.expr, ctx, unresolved_mark),
        _ => false,
    }
}

/// `ar || (ar = S)` or `!ar && (ar = S)`.
fn is_init_expr(expr: &Expr, ctx: &MatchContext, unresolved_mark: Option<Mark>) -> bool {
    let Expr::Bin(BinExpr {
        op, left, right, ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    let guard_matches = match op {
        BinaryOp::LogicalOr => ctx.is_binding(strip_parens(left), "ar"),
        BinaryOp::LogicalAnd => is_not_ar(left, ctx),
        _ => false,
    };
    guard_matches && is_slice_assign(right, ctx, unresolved_mark)
}

/// `ar = Array.prototype.slice.call(from, 0, i)`.
fn is_slice_assign(expr: &Expr, ctx: &MatchContext, unresolved_mark: Option<Mark>) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    assign.op == AssignOp::Assign
        && matches!(&assign.left, AssignTarget::Simple(SimpleAssignTarget::Ident(target))
            if ctx.is_ident(&target.id, "ar"))
        && is_slice_call(&assign.right, ctx, unresolved_mark, true)
}

/// `ar[i] = from[i]`.
fn is_copy_assign(expr: &Expr, ctx: &MatchContext) -> bool {
    let Expr::Assign(assign) = strip_parens(expr) else {
        return false;
    };
    let AssignTarget::Simple(SimpleAssignTarget::Member(target)) = &assign.left else {
        return false;
    };
    assign.op == AssignOp::Assign
        && is_index_of(target, "ar", ctx)
        && matches!(strip_parens(&assign.right), Expr::Member(source) if is_index_of(source, "from", ctx))
}

/// `<name>[i]`.
fn is_index_of(member: &MemberExpr, name: &str, ctx: &MatchContext) -> bool {
    ctx.is_binding(strip_parens(&member.obj), name)
        && matches!(&member.prop, MemberProp::Computed(computed) if ctx.is_binding(strip_parens(&computed.expr), "i"))
}

/// `Array.prototype.slice.call(from)`, or `.call(from, 0, i)` for a prefix.
fn is_slice_call(
    expr: &Expr,
    ctx: &MatchContext,
    unresolved_mark: Option<Mark>,
    prefix: bool,
) -> bool {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = strip_parens(expr)
    else {
        return false;
    };
    let Some(slice) = static_member(callee, "call") else {
        return false;
    };
    let is_array_prototype_slice = static_member(slice, "slice")
        .and_then(|prototype| static_member(prototype, "prototype"))
        .is_some_and(|array| {
            matches!(array, Expr::Ident(id) if is_unresolved_or_unguarded_ident(id, "Array", unresolved_mark))
        });
    if !is_array_prototype_slice || args.iter().any(|arg| arg.spread.is_some()) {
        return false;
    }
    match (prefix, args.as_slice()) {
        (false, [source]) => ctx.is_binding(strip_parens(&source.expr), "from"),
        (true, [source, start, end]) => {
            ctx.is_binding(strip_parens(&source.expr), "from")
                && matches!(strip_parens(&start.expr), Expr::Lit(Lit::Num(num)) if num.value == 0.0)
                && ctx.is_binding(strip_parens(&end.expr), "i")
        }
        _ => false,
    }
}

/// The object of `<object>.<name>`.
fn static_member<'a>(expr: &'a Expr, name: &str) -> Option<&'a Expr> {
    match strip_parens(expr) {
        Expr::Member(MemberExpr {
            obj,
            prop: MemberProp::Ident(prop),
            ..
        }) if prop.sym == name => Some(strip_parens(obj)),
        _ => None,
    }
}

/// `<to>.<method>(…)`: the call's arguments.
fn is_member_call<'a>(
    expr: &'a Expr,
    ctx: &MatchContext,
    object: &str,
    method: &str,
) -> Option<&'a [swc_core::ecma::ast::ExprOrSpread]> {
    let Expr::Call(CallExpr {
        callee: Callee::Expr(callee),
        args,
        ..
    }) = strip_parens(expr)
    else {
        return None;
    };
    let receiver = static_member(callee, method)?;
    ctx.is_binding(receiver, object).then_some(args.as_slice())
}

fn is_or(expr: &Expr, left: impl Fn(&Expr) -> bool, right: impl Fn(&Expr) -> bool) -> bool {
    matches!(strip_parens(expr), Expr::Bin(BinExpr { op: BinaryOp::LogicalOr, left: l, right: r, .. })
        if left(strip_parens(l)) && right(strip_parens(r)))
}
