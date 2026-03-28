mod common;

use wakaru_rs::rules::UnTypeof;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnTypeof)
}

#[test]
fn transforms_minified_typeof_patterns() {
    // Reused from packages/unminify/src/transformations/__tests__/un-typeof.spec.ts
    let input = r#"
typeof x < "u";
"u" > typeof x;
typeof x > "u";
"u" < typeof x;
"#;
    let expected = r#"
typeof x !== "undefined";
typeof x !== "undefined";
typeof x === "undefined";
typeof x === "undefined";
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_transform_other_typeof_comparisons() {
    // Reused from packages/unminify/src/transformations/__tests__/un-typeof.spec.ts
    let input = r#"
typeof x <= "u";
typeof x >= "u";
typeof x === "string";
typeof x === "number";
typeof x === "boolean";
typeof x === "symbol";
typeof x === "object";
typeof x === "bigint";
typeof x === "function";
typeof x === "undefined";
"#;

    let output = apply(input);
    assert_eq_normalized(&output, input);
}


