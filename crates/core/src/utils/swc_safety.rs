use std::panic::{self, AssertUnwindSafe};

use anyhow::anyhow;
use swc_core::common::DUMMY_SP;
use swc_core::ecma::ast::{
    AssignTarget, CallExpr, Callee, Class, Expr, ExprStmt, Module, NewExpr, OptCall, OptChainBase,
    ParenExpr, SimpleAssignTarget, TaggedTpl,
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
        parenthesize(callee);
    } else {
        parenthesize_leading_function_callee(callee);
    }
}

struct LeftHandSideOperandParens;

/// A position that takes a left-hand-side or member expression.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeftHandSidePosition {
    Callee,
    OptionalCallee,
    NewCallee,
    Heritage,
    Tag,
}

impl VisitMut for LeftHandSideOperandParens {
    fn visit_mut_call_expr(&mut self, node: &mut CallExpr) {
        node.visit_mut_children_with(self);
        if let Callee::Expr(callee) = &mut node.callee {
            parenthesize_left_hand_side_operand(callee, LeftHandSidePosition::Callee);
        }
    }

    fn visit_mut_opt_call(&mut self, node: &mut OptCall) {
        node.visit_mut_children_with(self);
        parenthesize_left_hand_side_operand(&mut node.callee, LeftHandSidePosition::OptionalCallee);
    }

    fn visit_mut_new_expr(&mut self, node: &mut NewExpr) {
        node.visit_mut_children_with(self);
        parenthesize_left_hand_side_operand(&mut node.callee, LeftHandSidePosition::NewCallee);
    }

    fn visit_mut_class(&mut self, node: &mut Class) {
        node.visit_mut_children_with(self);
        if let Some(super_class) = &mut node.super_class {
            parenthesize_left_hand_side_operand(super_class, LeftHandSidePosition::Heritage);
        }
    }

    fn visit_mut_tagged_tpl(&mut self, node: &mut TaggedTpl) {
        node.visit_mut_children_with(self);
        parenthesize_left_hand_side_operand(&mut node.tag, LeftHandSidePosition::Tag);
    }
}

/// Wrap an operand that a left-hand-side position cannot hold without
/// parentheses, for the kinds SWC's fixer leaves bare there. Its member-object
/// list is complete; the lists for the other positions are not:
///
/// - An update or arrow operand does not parse in any of these positions
///   (`x++()`, `new ()=>{}()`, `x++`t``), and `++x()` reparses as `++(x())`.
/// - A tag drops `await` and `yield` (`await a`t`` tags `a`, then awaits) and
///   a class (`class {}`t`` at a statement's start is a declaration).
/// - A call drops an argument-less `new`: `new a()` constructs `a` instead of
///   calling the constructed value.
/// - An optional call keeps no `new` callee bare: `new a()?.()` is valid, but
///   SWC's emitter asserts against printing it.
fn parenthesize_left_hand_side_operand(expression: &mut Box<Expr>, position: LeftHandSidePosition) {
    use LeftHandSidePosition::{Callee, OptionalCallee, Tag};

    let needs_parens = match expression.as_ref() {
        Expr::Update(_) | Expr::Arrow(_) => true,
        Expr::Await(_) | Expr::Yield(_) | Expr::Class(_) => position == Tag,
        Expr::New(new) => match position {
            Callee => new.args.is_none(),
            OptionalCallee => true,
            _ => false,
        },
        _ => false,
    };
    if needs_parens {
        parenthesize(expression);
    }
}

fn parenthesize(expression: &mut Box<Expr>) {
    let inner = std::mem::replace(expression, Box::new(Expr::Invalid(Default::default())));
    **expression = Expr::Paren(ParenExpr {
        span: DUMMY_SP,
        expr: inner,
    });
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
        // SWC's fixer also drops parentheses that a call or `new` callee, a
        // `class ... extends` clause, or a template tag needs, so those print
        // as `x++()` or `new o.n++()`. The positions take a left-hand-side
        // expression, so the repair is not limited to a statement's left edge.
        module.visit_mut_with(&mut LeftHandSideOperandParens);
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

    fn assert_fixed_output(source: &str, strip_parens: bool, expected: &str) {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let mut module = crate::unpacker::parse_es_module(source, "input.js", cm.clone())
                .expect("fixture should parse");
            if strip_parens {
                module.visit_mut_with(&mut StripParens);
            }

            apply_fixer(&mut module).expect("fixer should succeed");
            let output = crate::unpacker::emit_esm::emit_module_raw(&module, cm)
                .expect("fixture should emit");

            assert!(output.contains(expected), "unexpected output:\n{output}");
        });
    }

    // SWC's fixer used to give a JSX expression container the context of the
    // enclosing statement and drop the parentheses of `{(a, b)}`, printing
    // invalid JSX. Fixed upstream in swc_core 82; these cases guard against a
    // regression. https://github.com/swc-project/swc/issues/12484
    #[test]
    fn fixer_keeps_jsx_container_sequence_parens() {
        let source = r#"function f() { return <div a={(x, y)}>{(a, b)}</div>; }"#;
        assert_fixed_output(source, false, "<div a={(x, y)}>{(a, b)}</div>");
    }

    #[test]
    fn fixer_repairs_jsx_container_sequences_without_parens() {
        let source = r#"function f() { return <div a={(x, y)}>{(a, b)}{...(c, d)}</div>; }"#;
        assert_fixed_output(source, true, "<div a={(x, y)}>{(a, b)}{...(c, d)}</div>");
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

    // A call callee, a `new` callee, and a `class ... extends` clause each take
    // a left-hand-side expression, so an update or arrow operand keeps its
    // parentheses. SWC's fixer drops them, printing `x++()`, `new o.n++()`, and
    // `class C extends ()=>{}`.
    #[test]
    fn fixer_keeps_update_and_arrow_parens_in_left_hand_side_positions() {
        let source = r#"
(x++)();
new (o.n++)();
new (() => {})();
class C extends (() => {}) {}
"#;
        assert_fixed_output(source, false, "(x++)();");
        assert_fixed_output(source, false, "new (o.n++)();");
        assert_fixed_output(source, false, "new (()=>{})();");
        assert_fixed_output(source, false, "class C extends (()=>{})");
    }

    #[test]
    fn fixer_repairs_update_and_arrow_operands_without_source_parens() {
        let source = r#"
(x++)();
new (o.n++)();
new (() => {})();
class C extends (() => {}) {}
"#;
        assert_fixed_output(source, true, "(x++)();");
        assert_fixed_output(source, true, "new (o.n++)();");
        assert_fixed_output(source, true, "new (()=>{})();");
        assert_fixed_output(source, true, "class C extends (()=>{})");

        roundtrips_after_paren_strip(source);
    }

    #[test]
    fn fixer_keeps_parens_for_prefix_update_async_arrow_and_optional_call() {
        let cases = [
            ("(++x)();", "(++x)();"),
            ("(x--)();", "(x--)();"),
            ("new (++x)();", "new (++x)();"),
            ("new (async () => {})();", "new (async ()=>{})();"),
            (
                "class C extends (async () => {}) {}",
                "class C extends (async ()=>{})",
            ),
            ("(x++)?.();", "(x++)?.();"),
        ];
        for (source, expected) in cases {
            assert_fixed_output(source, false, expected);
            roundtrips_after_paren_strip(source);
        }
    }

    #[test]
    fn fixer_keeps_update_and_arrow_parens_in_nested_positions() {
        let source = r#"
var wrapped = ((x++)(), (0, y)());
keep([f((y++)())]);
"#;
        assert_fixed_output(source, false, "((x++)(),");
        assert_fixed_output(source, false, "f((y++)())");
        roundtrips_after_paren_strip(source);
    }

    // A template tag is a member-expression position too. The fixer drops the
    // parentheses of an update (which then does not parse), of `await` and
    // `yield` (which then apply to the tagged template), and of a class (which
    // then starts a declaration at a statement's start).
    #[test]
    fn fixer_keeps_tag_parens_the_fixer_drops() {
        let cases = [
            ("(x++)`t`;", "(x++)`t`;"),
            ("(++x)`t`;", "(++x)`t`;"),
            ("y = [f((o.n--)`t`)];", "f((o.n--)`t`)"),
            ("(class {})`t`;", "(class {\n})`t`;"),
            (
                "export default (class {})`t`;",
                "export default (class {\n})`t`;",
            ),
            ("async function f() { (await a)`t`; }", "(await a)`t`;"),
            ("function* g() { (yield a)`t`; }", "(yield a)`t`;"),
        ];
        for (source, expected) in cases {
            assert_fixed_output(source, false, expected);
            assert_fixed_output(source, true, expected);
        }
    }

    // `new a()` constructs `a`, while `(new a)()` calls the value `new a`
    // constructed. The fixer keeps the parentheses on a member object but not
    // on a callee.
    #[test]
    fn fixer_keeps_argumentless_new_callee_parens() {
        for strip_parens in [false, true] {
            assert_fixed_output("(new a)();", strip_parens, "(new a)();");
            assert_fixed_output("y = f((new a)());", strip_parens, "f((new a)())");
        }
        // With an argument list the callee needs no parentheses.
        assert_fixed_output("(new a())();", false, "new a()();");
    }

    // `new a()?.()` is valid, but SWC's emitter asserts that an optional
    // call's callee is not a `new` expression, so the parentheses stay.
    #[test]
    fn fixer_keeps_new_parens_in_optional_call_callee() {
        for strip_parens in [false, true] {
            assert_fixed_output("(new a)?.();", strip_parens, "(new a)?.();");
            assert_fixed_output("(new a())?.();", strip_parens, "(new a())?.();");
        }
    }

    #[test]
    fn fixer_does_not_parenthesize_valid_left_hand_side_operands() {
        GLOBALS.set(&Default::default(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let source = r#"
foo();
new Foo();
class D extends Foo {}
o.m++;
"#;
            let mut module = crate::unpacker::parse_es_module(source, "input.js", cm.clone())
                .expect("fixture should parse");

            apply_fixer(&mut module).expect("fixer should succeed");
            let output = crate::unpacker::emit_esm::emit_module_raw(&module, cm)
                .expect("fixture should emit");

            let expected = "foo();\nnew Foo();\nclass D extends Foo {\n}\no.m++;";
            assert_eq!(output.trim(), expected, "unexpected output:\n{output}");
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
