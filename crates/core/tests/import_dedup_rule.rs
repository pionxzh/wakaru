mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::ImportDedup;

#[test]
fn removes_exact_duplicate_named_imports() {
    let input = r#"
import { foo } from "pkg";
import { foo } from "pkg";
use(foo);
"#;
    let expected = r#"
import { foo } from "pkg";
use(foo);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), expected);
}

#[test]
fn removes_duplicate_named_import_and_renames_uses() {
    let input = r#"
import { foo } from "pkg";
import { foo as bar } from "pkg";
use(foo, bar);
"#;
    let expected = r#"
import { foo } from "pkg";
use(foo, foo);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), expected);
}
