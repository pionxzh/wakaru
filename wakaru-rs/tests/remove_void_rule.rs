mod common;

use common::{assert_compact_eq, normalize, render};

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
    assert_compact_eq(&output, expected);
}

#[test]
fn transforms_void_numeric_literals() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
void 0
void 99
void(0)
"#;
    let output = render(input);
    let normalized = normalize(&output);
    assert_eq!(normalized, "undefined; undefined; undefined;");
}

#[test]
fn does_not_transform_void_function_call() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
void function() {
  console.log('a')
  return void a()
}
"#;
    let expected = r#"
void function() {
  console.log('a');
  a();
};
"#;

    let output = render(input);
    assert_compact_eq(&output, expected);
}

#[test]
fn does_not_transform_when_undefined_is_declared() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
var undefined = 42;

console.log(void 0);

if (undefined !== a) {
  console.log('a', void 0);
}
"#;
    let expected = r#"
var undefined = 42;
console.log(void 0);
if (a !== undefined) {
  console.log('a', void 0);
}
"#;

    let output = render(input);
    assert_compact_eq(&output, expected);
}
