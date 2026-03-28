mod common;

use wakaru_rs::rules::UnReturn;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnReturn)
}

#[test]
fn transforms_return_void_expr_to_expression_statement() {
    // Reused from packages/unminify/src/transformations/__tests__/un-return.spec.ts
    let input = r#"
function foo() {
  return void a()
}
"#;
    let expected = r#"
function foo() {
  a();
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
    let expected = r#"
function foo() {
  const a = 1;
}

const bar = ()=>{
  const a = 1;
  if (a) return void 0;
};
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
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
    let expected = r#"
function foo() {
  const count = 5;
  while (count--) {
    return void 0;
  }

  for(let i = 0; i < 10; i++){
    return void foo();
  }
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}


