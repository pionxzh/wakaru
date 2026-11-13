//! Keep a string-literal expression statement that is not a directive from
//! turning into one.
//!
//! A string-literal expression statement is a directive only when it is
//! unparenthesized and sits in a body's directive prologue, before any other
//! statement. Two things can move a plain string statement there:
//!
//! - a rewrite removes the statements before it (`var x; "use strict";` loses
//!   the unused `var x;`), and
//! - SWC's fixer drops the parentheses of `("use strict");` as redundant.
//!
//! The string then prints as a directive, and a `"use strict"` makes the
//! function strict. Wrapping it in parentheses keeps it an ordinary
//! expression statement.

use swc_core::common::{BytePos, Span, DUMMY_SP};
use swc_core::ecma::ast::{Expr, ExprStmt, FunctionBody, Lit, Module, ModuleItem, ParenExpr, Stmt};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

use crate::collections::HashSet;
use crate::utils::paren::strip_parens;

/// Spans of string literals, keyed by position.
pub(crate) type StringSpans = HashSet<(BytePos, BytePos)>;

enum StringStatement {
    /// `"text";`: a directive when it is in the prologue.
    Bare(Span),
    /// `("text");`: never a directive, and it ends the prologue.
    Parenthesized(Span),
}

fn classify(stmt: &Stmt) -> Option<StringStatement> {
    let Stmt::Expr(ExprStmt { expr, .. }) = stmt else {
        return None;
    };
    match expr.as_ref() {
        Expr::Lit(Lit::Str(string)) => Some(StringStatement::Bare(string.span)),
        Expr::Paren(_) => match strip_parens(expr) {
            Expr::Lit(Lit::Str(string)) => Some(StringStatement::Parenthesized(string.span)),
            _ => None,
        },
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

/// Every string-literal expression statement in `module`, parenthesized or
/// not, that is not a directive.
pub(crate) fn collect_non_directive_strings(module: &mut Module) -> StringSpans {
    struct AllStrings(StringSpans);

    impl VisitMut for AllStrings {
        fn visit_mut_stmt(&mut self, stmt: &mut Stmt) {
            stmt.visit_mut_children_with(self);
            if let Some(StringStatement::Bare(span) | StringStatement::Parenthesized(span)) =
                classify(stmt)
            {
                self.0.extend(key(span));
            }
        }
    }

    let mut all = AllStrings(StringSpans::default());
    module.visit_mut_with(&mut all);
    let mut strings = all.0;
    if strings.is_empty() {
        return strings;
    }

    module.visit_mut_with(&mut Bodies(|stmts: &mut dyn Iterator<Item = &mut Stmt>| {
        for stmt in stmts {
            let Some(StringStatement::Bare(span)) = classify(stmt) else {
                break;
            };
            if let Some(key) = key(span) {
                strings.remove(&key);
            }
        }
    }));
    strings
}

/// Spans of the parenthesized strings that end a directive prologue. SWC's
/// fixer drops those parentheses, so collect them before it runs and restore
/// them with [`parenthesize_non_directive_strings`] after.
pub(crate) fn collect_prologue_parenthesized_strings(module: &mut Module) -> StringSpans {
    let mut strings = StringSpans::default();
    module.visit_mut_with(&mut Bodies(|stmts: &mut dyn Iterator<Item = &mut Stmt>| {
        for stmt in stmts {
            match classify(stmt) {
                Some(StringStatement::Bare(_)) => {}
                Some(StringStatement::Parenthesized(span)) => {
                    strings.extend(key(span));
                    break;
                }
                None => break,
            }
        }
    }));
    strings
}

/// Parenthesize the first unparenthesized string in each directive prologue
/// whose span is in `non_directives`. That string ends the prologue, so the
/// strings after it stay ordinary statements too.
pub(crate) fn parenthesize_non_directive_strings(
    module: &mut Module,
    non_directives: &StringSpans,
) {
    if non_directives.is_empty() {
        return;
    }
    module.visit_mut_with(&mut Bodies(|stmts: &mut dyn Iterator<Item = &mut Stmt>| {
        for stmt in stmts {
            let Some(StringStatement::Bare(span)) = classify(stmt) else {
                break;
            };
            if key(span).is_some_and(|key| non_directives.contains(&key)) {
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
