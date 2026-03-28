mod common;

use common::{assert_eq_normalized, render};

#[test]
fn transforms_void_zero_in_comparison() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
if(void 0 !== a) {
  console.log('a')
}
"#;
    let expected = r#"
if (a !== undefined) {
  console.log('a');
}
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn transforms_void_numeric_literals() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
const a = void 0;
const b = void 99;
const c = void(0);
"#;
    let expected = r#"
const a = undefined;
const b = undefined;
const c = undefined;
"#;
    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_void_function_call() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    // ArrowFunction rule converts the function expression to an arrow function.
    let input = r#"
const x = void function() {
  console.log('a');
  return void a();
};
"#;
    let expected = r#"
const x = void (()=>{
  console.log('a');
  a();
});
"#;

    let output = render(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_when_undefined_is_declared() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    // VarDeclToLetConst converts `var undefined = 42` to `const` since it's never reassigned.
    let input = r#"
var undefined = 42;

console.log(void 0);

if (undefined !== a) {
  console.log('a', void 0);
}
"#;
    let expected = r#"
const undefined = 42;
console.log(void 0);
if (a !== undefined) {
  console.log('a', void 0);
}
"#;

    let output = render(input);
    assert_eq_normalized(&output, expected);
}

