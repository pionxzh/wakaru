use swc_core::ecma::ast::{CallExpr, Expr, ExprOrSpread, Module, NewExpr, OptCall};
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

pub fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => strip_parens(&paren.expr),
        _ => expr,
    }
}

pub fn strip_parens_owned(expr: Expr) -> Expr {
    match expr {
        Expr::Paren(paren) => strip_parens_owned(*paren.expr),
        other => other,
    }
}

pub fn strip_parens_mut(expr: &mut Box<Expr>) -> &mut Expr {
    let mut current = expr.as_mut();
    loop {
        match current {
            Expr::Paren(paren) => current = paren.expr.as_mut(),
            _ => return current,
        }
    }
}

/// Remove the parentheses around a function or arrow passed as a call
/// argument: `f((function () {}))` becomes `f(function () {})`.
///
/// SWC keeps source parentheses as `Expr::Paren`, and the matchers that read
/// a function argument expect the bare function. Some producers wrap every
/// function argument: Terser before 5.43 does so by default (its
/// `wrap_func_args` output option), and rolldown wraps each lazy factory it
/// passes to `__commonJSMin`. The input parsers run this once, before
/// detection or any rule, so no matcher has to strip them itself.
///
/// Only this position is touched. A non-spread call or `new` argument is
/// parsed as an assignment expression, which a function or arrow already is,
/// so the parentheses change nothing and the printer's fixer never needs them
/// back. Parentheses elsewhere can carry meaning and stay: `(a?.b).c` is not
/// `a?.b.c`, and `("use strict");` is not a directive. A parenthesized
/// function callee (`(function () {})()`) also stays; it is the usual way to
/// write an IIFE, and the IIFE matchers already read through it.
pub(crate) fn unwrap_parenthesized_function_args(module: &mut Module) {
    module.visit_mut_with(&mut FunctionArgParens);
}

struct FunctionArgParens;

impl VisitMut for FunctionArgParens {
    fn visit_mut_call_expr(&mut self, call: &mut CallExpr) {
        call.visit_mut_children_with(self);
        unwrap_function_args(&mut call.args);
    }

    fn visit_mut_new_expr(&mut self, new: &mut NewExpr) {
        new.visit_mut_children_with(self);
        if let Some(args) = &mut new.args {
            unwrap_function_args(args);
        }
    }

    fn visit_mut_opt_call(&mut self, call: &mut OptCall) {
        call.visit_mut_children_with(self);
        unwrap_function_args(&mut call.args);
    }
}

fn unwrap_function_args(args: &mut [ExprOrSpread]) {
    for arg in args {
        if arg.spread.is_none()
            && matches!(*arg.expr, Expr::Paren(_))
            && matches!(strip_parens(&arg.expr), Expr::Fn(_) | Expr::Arrow(_))
        {
            let inner = std::mem::replace(&mut *arg.expr, Expr::Invalid(Default::default()));
            *arg.expr = strip_parens_owned(inner);
        }
    }
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};
    use swc_core::ecma::ast::{Callee, Decl, MemberExpr, ModuleItem, Stmt};
    use swc_core::ecma::visit::{Visit, VisitWith};

    use super::*;

    /// Counts what the parsed module still holds: parenthesized call
    /// arguments by inner kind, and the parentheses around a callee or a
    /// member object.
    #[derive(Default)]
    struct ParenCounts {
        function_args: usize,
        other_args: usize,
        callees: usize,
        member_objects: usize,
    }

    impl Visit for ParenCounts {
        fn visit_expr_or_spread(&mut self, arg: &ExprOrSpread) {
            if matches!(*arg.expr, Expr::Paren(_)) {
                if matches!(strip_parens(&arg.expr), Expr::Fn(_) | Expr::Arrow(_)) {
                    self.function_args += 1;
                } else {
                    self.other_args += 1;
                }
            }
            arg.visit_children_with(self);
        }

        fn visit_callee(&mut self, callee: &Callee) {
            if matches!(callee, Callee::Expr(expr) if matches!(**expr, Expr::Paren(_))) {
                self.callees += 1;
            }
            callee.visit_children_with(self);
        }

        fn visit_member_expr(&mut self, member: &MemberExpr) {
            if matches!(*member.obj, Expr::Paren(_)) {
                self.member_objects += 1;
            }
            member.visit_children_with(self);
        }
    }

    fn parse(source: &str) -> Module {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            crate::unpacker::parse_es_module(source, "input.js", cm).expect("source should parse")
        })
    }

    #[test]
    fn input_parse_unwraps_function_arguments_only() {
        let module = parse(
            r#"
            f((function () {}), ((() => 1)), (async function* () {}));
            new F((function () {}));
            o?.m((x => x));
            outer(inner((function () {})));
            g((x, y), (h), ...(rest));
            (function () {})();
            (a?.b).c;
            "#,
        );
        let mut counts = ParenCounts::default();
        module.visit_with(&mut counts);

        assert_eq!(counts.function_args, 0);
        assert_eq!(
            counts.other_args, 3,
            "`(x, y)`, `(h)`, and the spread `(rest)` keep theirs"
        );
        assert_eq!(counts.callees, 1, "the IIFE callee keeps its parentheses");
        assert_eq!(counts.member_objects, 1, "`(a?.b).c` keeps its parentheses");
    }

    #[test]
    fn input_parse_keeps_a_parenthesized_string_statement() {
        let module = parse(r#"("use strict"); function f() { ("use strict"); }"#);
        let ModuleItem::Stmt(Stmt::Expr(first)) = &module.body[0] else {
            panic!("expected an expression statement");
        };
        assert!(matches!(*first.expr, Expr::Paren(_)));
        let ModuleItem::Stmt(Stmt::Decl(Decl::Fn(function))) = &module.body[1] else {
            panic!("expected a function declaration");
        };
        let body = function.function.body.as_ref().expect("function body");
        assert!(matches!(&body.stmts[0], Stmt::Expr(stmt) if matches!(*stmt.expr, Expr::Paren(_))));
    }
}
