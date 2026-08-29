use std::panic::{self, AssertUnwindSafe};

use anyhow::anyhow;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignTarget, Callee, Expr, ExprStmt, Module, OptChainBase, ParenExpr, SimpleAssignTarget,
};
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
            parenthesize_callee(callee);
        }
        Expr::Member(member) => parenthesize_leading_function_callee(&mut member.obj),
        Expr::TaggedTpl(tagged) => parenthesize_leading_function_callee(&mut tagged.tag),
        // An assignment's target chain is the statement's left edge; the
        // right-hand side is not.
        Expr::Assign(assign) => {
            if let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &mut assign.left {
                parenthesize_leading_function_callee(&mut member.obj);
            }
        }
        Expr::OptChain(chain) => match &mut *chain.base {
            OptChainBase::Member(member) => parenthesize_leading_function_callee(&mut member.obj),
            OptChainBase::Call(call) => parenthesize_callee(&mut call.callee),
        },
        // Only a postfix operand sits at the statement's left edge; a prefix
        // operator already disambiguates the prefix.
        Expr::Update(update) => {
            if !update.prefix {
                parenthesize_leading_function_callee(&mut update.arg);
            }
        }
        // Existing parentheses already make the expression-statement prefix
        // unambiguous, so there is nothing to repair below this point.
        Expr::Paren(_) => {}
        _ => {}
    }
}

fn parenthesize_callee(callee: &mut Box<Expr>) {
    if matches!(callee.as_ref(), Expr::Fn(_) | Expr::Class(_)) {
        let expression = std::mem::replace(callee, Box::new(Expr::Invalid(Default::default())));
        **callee = Expr::Paren(ParenExpr {
            span: DUMMY_SP,
            expr: expression,
        });
    } else {
        parenthesize_leading_function_callee(callee);
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

    /// Strip every Paren node, simulating a transform that rebuilt the tree
    /// without source parentheses, so the repair path itself is exercised.
    struct StripParens;

    impl VisitMut for StripParens {
        fn visit_mut_expr(&mut self, expression: &mut Expr) {
            expression.visit_mut_children_with(self);
            while let Expr::Paren(paren) = expression {
                *expression = *std::mem::replace(
                    &mut paren.expr,
                    Box::new(Expr::Invalid(Default::default())),
                );
            }
        }
    }

    fn roundtrips_after_paren_strip(source: &str) {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = crate::unpacker::parse_es_module(source, "input.js", cm.clone())
                .expect("fixture should parse");
            module.visit_mut_with(&mut StripParens);

            apply_fixer(&mut module).expect("fixer should succeed");
            let output = crate::unpacker::emit_esm::emit_module_raw(&module, cm.clone())
                .expect("fixture should emit");

            crate::unpacker::parse_es_module(&output, "output.js", cm).unwrap_or_else(|error| {
                panic!("fixed output should parse: {error}; output:\n{output}")
            });
        });
    }

    #[test]
    fn fixer_repairs_assignment_target_left_edges() {
        roundtrips_after_paren_strip(r#"(function() { return {}; })().prop = 1;"#);
    }

    #[test]
    fn fixer_repairs_optional_chain_left_edges() {
        roundtrips_after_paren_strip(r#"(function() { return {}; })()?.x;"#);
        roundtrips_after_paren_strip(r#"(function() { return {}; })?.();"#);
    }

    #[test]
    fn fixer_repairs_postfix_update_left_edges() {
        roundtrips_after_paren_strip(r#"(function() { return { count: 0 }; })().count++;"#);
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
