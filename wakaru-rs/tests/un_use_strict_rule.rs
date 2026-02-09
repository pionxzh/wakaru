mod common;

use common::normalize;
use common::render;

#[test]
fn removes_use_strict_directive() {
    // Reused from packages/unminify/src/transformations/__tests__/un-use-stict.spec.ts
    let input = r#"
'use strict'
"#;

    let output = render(input);
    assert_eq!(normalize(&output), "");
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

    let output = render(input);
    let normalized = normalize(&output);
    assert!(normalized.starts_with("function foo(str)"));
    assert!(normalized.contains("function foo(str) { return str === 'use strict'; }"));
}
