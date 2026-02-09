mod common;

use common::{assert_normalized_eq, render};

#[test]
fn flips_supported_literal_on_left() {
    // Reused subset from packages/unminify/src/transformations/__tests__/un-flip-comparisons.spec.ts
    let input = r#"
void 0 === foo;
undefined === foo;
null !== foo;
1 == foo;
true != foo;
"str" == foo;
`test` == foo;
NaN == foo;
Infinity == foo;
-Infinity == foo;

1 < bar;
1 > bar;
1 <= bar;
1 >= bar;
"#;
    let expected = r#"
foo === undefined;
foo === undefined;
foo !== null;
foo == 1;
foo != true;
foo == "str";
foo == `test`;
foo == NaN;
foo == Infinity;
foo == -Infinity;

bar > 1;
bar < 1;
bar >= 1;
bar <= 1;
"#;

    let output = render(input);
    assert_normalized_eq(&output, expected);
}

#[test]
fn flips_comparison_for_expression_right_hand_side() {
    // Reused from packages/unminify/src/transformations/__tests__/un-flip-comparisons.spec.ts
    let input = r#"
1 == obj.props;
1 == obj.props[0];
1 == method();
"#;
    let expected = r#"
obj.props == 1;
obj.props[0] == 1;
method() == 1;
"#;

    let output = render(input);
    assert_normalized_eq(&output, expected);
}

#[test]
fn does_not_flip_when_left_is_not_supported_literal_like() {
    // Reused subset from packages/unminify/src/transformations/__tests__/un-flip-comparisons.spec.ts
    let input = r#"
foo === undefined;
foo !== null;
foo == 1;
({}) == foo;
`test${1}` == foo;
bar > 1;
bar < 1.2;
"#;

    let output = render(input);
    assert_normalized_eq(&output, input);
}
