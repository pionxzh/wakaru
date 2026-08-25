use std::panic::{self, AssertUnwindSafe};

use anyhow::anyhow;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{Callee, Expr, ExprStmt, Module, ParenExpr};
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::visit::{VisitMut, VisitMutWith};

struct FunctionExpressionCalleeParens;

impl VisitMut for FunctionExpressionCalleeParens {
    fn visit_mut_expr_stmt(&mut self, statement: &mut ExprStmt) {
        statement.visit_mut_children_with(self);
        parenthesize_leading_function_callee(&mut statement.expr);
    }
}

fn parenthesize_leading_function_callee(expression: &mut Expr) {
    match expression {
        Expr::Bin(binary) => parenthesize_leading_function_callee(&mut binary.left),
        Expr::Cond(conditional) => parenthesize_leading_function_callee(&mut conditional.test),
        Expr::Seq(sequence) => {
            if let Some(first) = sequence.exprs.first_mut() {
                parenthesize_leading_function_callee(first);
            }
        }
        Expr::Call(call) => {
            let Callee::Expr(callee) = &mut call.callee else {
                return;
            };
            if matches!(callee.as_ref(), Expr::Fn(_) | Expr::Class(_)) {
                let expression =
                    std::mem::replace(callee, Box::new(Expr::Invalid(Default::default())));
                **callee = Expr::Paren(ParenExpr {
                    span: DUMMY_SP,
                    expr: expression,
                });
            } else {
                parenthesize_leading_function_callee(callee);
            }
        }
        Expr::Member(member) => parenthesize_leading_function_callee(&mut member.obj),
        Expr::TaggedTpl(tagged) => parenthesize_leading_function_callee(&mut tagged.tag),
        // Existing parentheses already make the expression-statement prefix
        // unambiguous, so there is nothing to repair below this point.
        Expr::Paren(_) => {}
        _ => {}
    }
}

/// Run SWC's fixer pass, catching panics from malformed AST that the
/// error-recovery parser accepted but the fixer doesn't handle.
pub(crate) fn apply_fixer(module: &mut Module) -> anyhow::Result<()> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        module.visit_mut_with(&mut fixer(None));
        // SWC's fixer can drop the source parentheses from an immediately
        // invoked function expression when the call is the left edge of a
        // larger expression statement. Its emitter then prints a forbidden
        // declaration-like `function (...) {}(...)` statement. Repair the
        // callee after the fixer so every downstream emission remains valid.
        // Limit the repair to the statement's left edge: the same callee in a
        // variable initializer or assignment RHS is already valid JavaScript.
        module.visit_mut_with(&mut FunctionExpressionCalleeParens);
    }))
    .map_err(|payload| {
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        anyhow!("SWC fixer panicked on malformed AST: {msg}")
    })
}

#[cfg(test)]
mod tests {
    use swc_core::common::{sync::Lrc, SourceMap, GLOBALS};

    use super::*;

    #[test]
    fn fixer_keeps_function_expression_callee_parseable() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let source = r#"
function accept(value) {
    (function(candidate) {
        return candidate != null;
    }(value) || fallback());
}
"#;
            let mut module = crate::unpacker::parse_es_module(source, "input.js", cm.clone())
                .expect("fixture should parse");

            apply_fixer(&mut module).expect("fixer should succeed");
            let output = crate::unpacker::emit_esm::emit_module_raw(&module, cm.clone())
                .expect("fixture should emit");

            crate::unpacker::parse_es_module(&output, "output.js", cm).unwrap_or_else(|error| {
                panic!("fixed output should parse: {error}; output:\n{output}")
            });
        });
    }

    #[test]
    fn fixer_does_not_parenthesize_valid_function_expression_callees() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let source = r#"
var initialized = function() {}();
initialized = function() {}();
"#;
            let mut module = crate::unpacker::parse_es_module(source, "input.js", cm.clone())
                .expect("fixture should parse");

            apply_fixer(&mut module).expect("fixer should succeed");
            let output = crate::unpacker::emit_esm::emit_module_raw(&module, cm)
                .expect("fixture should emit");

            assert!(
                !output.contains("(function"),
                "unexpected output:\n{output}"
            );
        });
    }
}
