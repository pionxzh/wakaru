mod common;

use common::{assert_eq_normalized, render};

// Verify that dead imports are stripped to side-effect-only form after the
// full pipeline has run. This runs via the default rules pipeline.

#[test]
fn unused_default_import_becomes_side_effect() {
    let input = r#"
import r from "./x.js";
const a = 1;
"#;
    let expected = r#"
import "./x.js";
const a = 1;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn used_default_import_is_kept() {
    let input = r#"
import r from "./x.js";
console.log(r);
"#;
    let expected = r#"
import r from "./x.js";
console.log(r);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn unused_named_specifier_is_stripped() {
    let input = r#"
import { a, b } from "./x.js";
console.log(a);
"#;
    let expected = r#"
import { a } from "./x.js";
console.log(a);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn mixed_default_and_named_partial_strip() {
    // Default is used; named `b` is not.
    let input = r#"
import R, { a, b } from "./x.js";
console.log(R, a);
"#;
    let expected = r#"
import R, { a } from "./x.js";
console.log(R, a);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn namespace_import_used_via_member_is_kept() {
    let input = r#"
import * as ns from "./x.js";
console.log(ns.foo);
"#;
    let expected = r#"
import * as ns from "./x.js";
console.log(ns.foo);
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn side_effect_only_import_is_kept() {
    let input = r#"
import "./x.js";
const a = 1;
"#;
    let expected = r#"
import "./x.js";
const a = 1;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn jsx_reference_counts_as_usage() {
    // `<Foo/>` is a reference to Foo. Must not strip it.
    let input = r#"
import Foo from "./x.js";
const e = <Foo/>;
"#;
    let expected = r#"
import Foo from "./x.js";
const e = <Foo/>;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn jsx_member_expression_counts_as_usage() {
    // `<r.Switch/>` references r even though only `.Switch` is visible.
    let input = r#"
import r from "./x.js";
const e = <r.Switch/>;
"#;
    let expected = r#"
import r from "./x.js";
const e = <r.Switch/>;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}

#[test]
fn all_specifiers_stripped_yields_side_effect_import() {
    let input = r#"
import r, { a, b } from "./x.js";
const c = 1;
"#;
    let expected = r#"
import "./x.js";
const c = 1;
"#;
    assert_eq_normalized(&render(input), expected.trim());
}
