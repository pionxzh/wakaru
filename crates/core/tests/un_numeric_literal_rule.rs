mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnNumericLiteral;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnNumericLiteral)
}

#[test]
fn transforms_numeric_literals_with_different_notation() {
    // Reused from packages/unminify/src/transformations/__tests__/un-numeric-literal.spec.ts
    // Using variable declarations so literals are in a meaningful context
    let input = r#"
const a = 65536;
const b = 123.4;
const c = 0b101010;
const d = 0o777;
const e = -0x123;
const f = 4.2e2;
const g = -2e4;
"#;
    let expected = r#"
const a = 65536;
const b = 123.4;
const c = 42;
const d = 511;
const e = -291;
const f = 420;
const g = -20000;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn preserves_non_finite_numeric_literal_spelling() {
    let input = r#"
const negative = -1e999;
const positive = 2e308;
const fraction = .75;
"#;
    let expected = r#"
const negative = -1e999;
const positive = 2e308;
const fraction = 0.75;
"#;

    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_the_quoted_raw_of_unpacks_kept_string_module_id() {
    // Unpack writes a kept webpack `require("q1tI")` as a number literal of
    // value -1 whose raw is the quoted id; dropping that raw would print -1.
    use swc_core::ecma::ast::{Expr, ExprStmt, Lit, ModuleItem, Number, Stmt};
    use swc_core::ecma::visit::VisitMutWith;

    let mut item = ModuleItem::Stmt(Stmt::Expr(ExprStmt {
        span: Default::default(),
        expr: Box::new(Expr::Lit(Lit::Num(Number {
            span: Default::default(),
            value: -1.0,
            raw: Some("\"q1tI\"".into()),
        }))),
    }));
    item.visit_mut_with(&mut UnNumericLiteral);
    let ModuleItem::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &item else {
        unreachable!()
    };
    let Expr::Lit(Lit::Num(number)) = &**expr else {
        unreachable!()
    };
    assert_eq!(number.raw.as_deref(), Some("\"q1tI\""));
}
