mod common;

use common::{assert_eq_normalized, render};

#[test]
fn basic_for_to_for_of() {
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(x); }"#;
    let expected = r#"for (const x of items) { console.log(x); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_with_block_body() {
    let input = r#"for (let Y = 0, V = list; Y < V.length; Y++) { const Z = V[Y]; if (Z != null) { process(Z); } }"#;
    let expected = r#"for (const Z of list) { if (Z != null) { process(Z); } }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn for_of_with_method_call_iterable() {
    let input = r#"for (let Y = 0, V = Object.keys(obj); Y < V.length; Y++) { const Z = V[Y]; use(Z); }"#;
    let expected = r#"for (const Z of Object.keys(obj)) { use(Z); }"#;
    assert_eq_normalized(&render(input), expected);
}

#[test]
fn no_transform_when_index_used_in_body() {
    // Index `i` is used beyond just arr[i], so can't convert
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(i, x); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_when_arr_used_in_body() {
    // arr variable used beyond arr[i] and arr.length
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { const x = arr[i]; console.log(arr.length, x); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_when_no_elem_decl() {
    // No `const elem = arr[i]` as first statement
    let input = r#"for (let i = 0, arr = items; i < arr.length; i++) { console.log(arr[i]); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn no_transform_regular_for_loop() {
    let input = r#"for (let i = 0; i < 10; i++) { console.log(i); }"#;
    assert_eq_normalized(&render(input), input);
}

#[test]
fn for_of_single_decl_arr_form() {
    // Variant: for(let i = 0; i < arr.length; i++) { const x = arr[i]; ... }
    // Only one declarator, arr is external — still transformable if arr is not modified
    let input = r#"for (let Y = 0, V = B.split("."); Y < V.length; Y++) { const Z = V[Y]; process(Z); }"#;
    let expected = r#"for (const Z of B.split(".")) { process(Z); }"#;
    assert_eq_normalized(&render(input), expected);
}
