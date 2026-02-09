mod common;

use common::{normalize, render};

#[test]
fn transforms_void_zero_in_comparison() {
    // Reused from packages/unminify/src/transformations/__tests__/un-undefined.spec.ts
    let input = r#"
if(void 0 !== a) {
  console.log('a')
}
"#;
    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("a !== undefined"));
    assert!(!normalized.contains("void 0"));
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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("void function()"));
    assert!(normalized.contains("return void a();"));
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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.contains("console.log(void 0);"));
    assert!(normalized.contains("console.log('a', void 0);"));
}
