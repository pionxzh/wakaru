//! Keep a string-literal expression statement that is not a directive from
//! turning into one.
//!
//! A string-literal expression statement is a directive only when it is
//! unparenthesized and sits in a body's directive prologue, before any other
//! statement. Several things can move a plain string there:
//!
//! - a rewrite removes the statements before it (`var x; "use strict";` loses
//!   the unused `var x;`),
//! - a rewrite splits a sequence (`"use strict", g();` becomes
//!   `"use strict"; g();`), and
//! - SWC's fixer drops the parentheses of `("use strict");` as redundant.
//!
//! The string then prints as a directive, and a `"use strict"` makes the
//! function strict. So record the directives before the change, and after it
//! wrap the first prologue string that was not one in parentheses, which keeps
//! it an ordinary expression statement and ends the prologue.

use swc_core::common::{BytePos, Span, DUMMY_SP};
use swc_core::ecma::ast::{Expr, ExprStmt, FunctionBody, Lit, Module, ModuleItem, ParenExpr, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::collections::HashSet;

/// Spans of directive string literals, keyed by position.
pub(crate) type DirectiveSpans = HashSet<(BytePos, BytePos)>;

/// The string of a bare `"text";` statement, the only form that can be a
/// directive. A parenthesized `("text");` never is, and it ends the prologue.
fn bare_string(stmt: &Stmt) -> Option<Span> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    match expr.as_ref() {
        Expr::Lit(Lit::Str(string)) => Some(string.span),
        _ => None,
    }
}

fn key(span: Span) -> Option<(BytePos, BytePos)> {
    (span != DUMMY_SP).then_some((span.lo, span.hi))
}

/// Calls the closure with the statements of every body that can hold a directive
/// prologue: the module top level and each function body. Class static blocks
/// are `BlockStmt`s and have no prologue.
struct Bodies<F>(F);

impl<F: FnMut(&mut dyn Iterator<Item = &mut Stmt>)> VisitMut for Bodies<F> {
    fn visit_mut_module(&mut self, module: &mut Module) {
        module.visit_mut_children_with(self);
        (self.0)(&mut module.body.iter_mut().map_while(ModuleItem::as_mut_stmt));
    }

    // Function, method, constructor, accessor, and block-bodied arrow bodies.
    fn visit_mut_function_body(&mut self, body: &mut FunctionBody) {
        body.visit_mut_children_with(self);
        (self.0)(&mut body.stmts.iter_mut());
    }
}

/// Spans of every directive in `module`: the bare strings at the start of each
/// body.
pub(crate) fn collect_directives(module: &mut Module) -> DirectiveSpans {
    let mut directives = DirectiveSpans::default();
    module.visit_mut_with(&mut Bodies(|stmts: &mut dyn Iterator<Item = &mut Stmt>| {
        for stmt in stmts {
            let Some(span) = bare_string(stmt) else {
                break;
            };
            directives.extend(key(span));
        }
    }));
    directives
}

/// Parenthesize the first bare string in each directive prologue that is not
/// in `directives`. That string ends the prologue, so the strings after it stay
/// ordinary statements too. A string without a source span was built by a
/// rewrite rather than moved, so it is left alone.
pub(crate) fn parenthesize_new_directives(module: &mut Module, directives: &DirectiveSpans) {
    module.visit_mut_with(&mut Bodies(|stmts: &mut dyn Iterator<Item = &mut Stmt>| {
        for stmt in stmts {
            let Some(span) = bare_string(stmt) else {
                break;
            };
            if key(span).is_some_and(|key| !directives.contains(&key)) {
                if let Stmt::Expr(statement) = stmt {
                    let string = std::mem::replace(
                        &mut statement.expr,
                        Box::new(Expr::Invalid(Default::default())),
                    );
                    *statement.expr = Expr::Paren(ParenExpr {
                        span: DUMMY_SP,
                        expr: string,
                    });
                }
                break;
            }
        }
    }));
}
