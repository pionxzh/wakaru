use swc_core::ecma::ast::{CallExpr, Callee, Expr, Ident, Lit, SeqExpr, WithStmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use super::RewriteLevel;

pub struct UnIndirectCall {
    level: RewriteLevel,
    with_depth: usize,
}

impl UnIndirectCall {
    pub fn new(level: RewriteLevel) -> Self {
        Self {
            level,
            with_depth: 0,
        }
    }
}

impl Default for UnIndirectCall {
    fn default() -> Self {
        Self::new(RewriteLevel::Standard)
    }
}

impl VisitMut for UnIndirectCall {
    fn visit_mut_with_stmt(&mut self, stmt: &mut WithStmt) {
        self.with_depth += 1;
        stmt.visit_mut_children_with(self);
        self.with_depth -= 1;
    }

    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        expr.visit_mut_children_with(self);

        let Expr::Call(CallExpr {
            span,
            ctxt,
            callee,
            args,
            type_args,
        }) = expr
        else {
            return;
        };

        let Callee::Expr(callee_expr) = callee else {
            return;
        };

        // Pattern 1: (0, fn)(args) → fn(args) is safe for direct identifiers,
        // except `eval` and `with`-scoped identifiers. Member calls stay
        // standard+ because they change the receiver `this` binding.
        if let Some(seq) = as_seq_expr(callee_expr) {
            let exprs = &seq.exprs;
            if exprs.len() == 2
                && matches!(&*exprs[0], Expr::Lit(Lit::Num(num)) if num.value == 0.0)
            {
                let inner = if self.level >= RewriteLevel::Standard {
                    as_member_or_safe_ident(&exprs[1], self.with_depth)
                } else {
                    as_safe_ident(&exprs[1], self.with_depth)
                };
                if let Some(inner) = inner {
                    *expr = Expr::Call(CallExpr {
                        span: *span,
                        ctxt: *ctxt,
                        callee: Callee::Expr(inner),
                        args: args.clone(),
                        type_args: type_args.clone(),
                    });
                }
            }
            return;
        }

        // Pattern 2: Object(fn.method)(args) → fn.method(args)
        // Object() called on a function just returns it — used as indirect call
        if self.level >= RewriteLevel::Standard {
            if let Some(inner) = as_object_wrap_call(callee_expr, self.with_depth) {
                *expr = Expr::Call(CallExpr {
                    span: *span,
                    ctxt: *ctxt,
                    callee: Callee::Expr(inner),
                    args: args.clone(),
                    type_args: type_args.clone(),
                });
            }
        }
    }
}

/// If `expr` is `Object(inner)` where inner is a member or ident expr, return `inner`.
fn as_object_wrap_call(expr: &Expr, with_depth: usize) -> Option<Box<Expr>> {
    let Expr::Call(call) = strip_paren(expr) else {
        return None;
    };
    if !call.args.is_empty() && call.args.len() != 1 {
        return None;
    }
    let Callee::Expr(callee_expr) = &call.callee else {
        return None;
    };
    // Must be exactly `Object`
    let Expr::Ident(Ident { sym, .. }) = strip_paren(callee_expr) else {
        return None;
    };
    if sym.as_str() != "Object" {
        return None;
    }
    let arg = call.args.first()?;
    if arg.spread.is_some() {
        return None;
    }
    as_member_or_safe_ident(&arg.expr, with_depth)
}

fn strip_paren(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_paren(&paren.expr),
        _ => expr,
    }
}

fn as_seq_expr(expr: &Expr) -> Option<&SeqExpr> {
    match strip_paren(expr) {
        Expr::Seq(seq) => Some(seq),
        _ => None,
    }
}

fn as_member_or_safe_ident(expr: &Expr, with_depth: usize) -> Option<Box<Expr>> {
    match strip_paren(expr) {
        Expr::Member(_) => Some(Box::new(strip_paren(expr).clone())),
        Expr::Ident(_) => as_safe_ident(expr, with_depth),
        _ => None,
    }
}

fn as_safe_ident(expr: &Expr, with_depth: usize) -> Option<Box<Expr>> {
    let Expr::Ident(id) = strip_paren(expr) else {
        return None;
    };
    if with_depth > 0 || id.sym.as_str() == "eval" {
        return None;
    }
    Some(Box::new(Expr::Ident(id.clone())))
}
