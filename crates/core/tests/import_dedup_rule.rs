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

#[test]
fn keeps_duplicate_import_when_canonical_name_is_shadowed_at_use() {
    let input = r#"
import primary from "pkg";
import alternate from "pkg";
function read() {
  use(alternate);
  const primary = makeLocal();
  return primary;
}
use(primary);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}

#[test]
fn keeps_duplicate_specifiers_with_different_import_attributes() {
    let input = r#"
import jsonValue from "./resource" with { type: "json" };
import cssValue from "./resource" with { type: "css" };
sink(jsonValue, cssValue);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}

#[test]
fn does_not_merge_imports_with_different_import_attributes() {
    let input = r#"
import { jsonValue } from "./resource" with { type: "json" };
import { cssValue } from "./resource" with { type: "css" };
sink(jsonValue, cssValue);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}

#[test]
fn does_not_merge_imports_with_different_phases() {
    let input = r#"
import source sourceValue from "./resource";
import { evaluatedValue } from "./resource";
sink(sourceValue, evaluatedValue);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}

#[test]
fn keeps_duplicate_specifiers_with_different_phases() {
    let input = r#"
import source sourceValue from "./resource";
import evaluatedValue from "./resource";
sink(sourceValue, evaluatedValue);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}

#[test]
fn merges_imports_with_identical_request_metadata() {
    let input = r#"
import { first } from "./resource" with { type: "json" };
import { second } from "./resource" with { type: "json" };
sink(first, second);
"#;
    let expected = r#"
import { first, second } from "./resource" with { type: "json" };
sink(first, second);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), expected);
}

#[test]
fn invalid_duplicate_import_attribute_keys_fail_closed() {
    let input = r#"
import { a } from "m" with { type: "json", type: "css" };
import { b } from "m" with { type: "json", type: "css" };
use(a, b);
"#;
    assert_eq_normalized(&render_rule(input, |_| ImportDedup), input);
}
