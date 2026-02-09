mod common;

use common::{normalize, render};

#[test]
fn transforms_return_void_expr_to_expression_statement() {
    // Reused from packages/unminify/src/transformations/__tests__/un-return.spec.ts
    let input = r#"
function foo() {
  return void a()
}
"#;
    let output = render(input);
    assert!(normalize(&output).contains("function foo() { a(); }"));
}

#[test]
fn removes_redundant_tail_return() {
    // Reused from packages/unminify/src/transformations/__tests__/un-return.spec.ts
    let input = r#"
function foo() {
  const a = 1
  return undefined
}

const bar = () => {
  const a = 1
  if (a) return void 0
  return void 0
}
"#;

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("function foo() { const a = 1; }"));
    assert!(normalized.contains("const bar = ()=>{ const a = 1; if (a) return undefined; };"));
}

#[test]
fn does_not_transform_non_tail_returns() {
    // Reused from packages/unminify/src/transformations/__tests__/un-return.spec.ts
    let input = r#"
function foo() {
  const count = 5;
  while (count--) {
    return void 0;
  }

  for (let i = 0; i < 10; i++) {
    return void foo();
  }
}
"#;
    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("return undefined;"));
    assert!(normalized.contains("return void foo();"));
}
