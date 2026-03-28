mod common;

use wakaru_rs::rules::UnUseStrict;
use common::{assert_eq_normalized, render_rule};

fn apply(input: &str) -> String {
    render_rule(input, |_| UnUseStrict)
}

#[test]
fn removes_use_strict_directive() {
    // Reused from packages/unminify/src/transformations/__tests__/un-use-stict.spec.ts
    let input = r#"
'use strict'
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn removes_use_strict_in_nested_function() {
    // Reused from packages/unminify/src/transformations/__tests__/un-use-stict.spec.ts
    let input = r#"
// comment
// another comment
'use strict'
function foo(str) {
  'use strict'
  return str === 'use strict'
}
"#;
    let expected = r#"
function foo(str) {
  return str === 'use strict';
}
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_non_directive_use_strict_string_literals() {
    let input = r#"
function foo() {
  a();
  'use strict';
  return 1;
}
"#;
    let expected = r#"
function foo() {
  a();
  'use strict';
  return 1;
}
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}


