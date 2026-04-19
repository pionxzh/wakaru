mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnImportRename;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnImportRename)
}

#[test]
fn renames_short_import_alias_to_original() {
    let input = r#"
import { foo as ab } from 'bar';
ab();
"#;
    let expected = r#"
import { foo } from 'bar';
foo();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn renames_multiple_import_aliases() {
    let input = r#"
import { alpha as a, beta as b } from 'mod';
a();
b();
"#;
    let expected = r#"
import { alpha, beta } from 'mod';
alpha();
beta();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_already_shorthand_import() {
    let input = r#"
import { foo } from 'bar';
foo();
"#;
    let expected = r#"
import { foo } from 'bar';
foo();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generates_unique_name_on_conflict() {
    // 'foo' is already declared locally — should rename to 'foo_1'
    let input = r#"
import { foo as ab } from 'bar';
const foo = 42;
ab();
"#;
    let expected = r#"
import { foo as foo_1 } from 'bar';
const foo = 42;
foo_1();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_alias_when_imported_name_conflicts_with_default_import() {
    let input = r#"
import a from './module-12.js';
import { a as a_2 } from './module-9.js';
a.a();
a_2.fixed();
"#;
    let expected = r#"
import a from './module-12.js';
import { a as a_1 } from './module-9.js';
a.a();
a_1.fixed();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn keeps_default_and_namespace_imports_unchanged() {
    let input = r#"
import defaultExport from 'mod';
import * as ns from 'mod';
defaultExport();
ns.foo();
"#;
    let expected = r#"
import defaultExport from 'mod';
import * as ns from 'mod';
defaultExport();
ns.foo();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn import_rename_does_not_touch_shadowed_local() {
    let input = r#"
import { foo as ab } from 'bar';
function inner() {
  const ab = 1;
  return ab;
}
ab();
"#;
    let expected = r#"
import { foo } from 'bar';
function inner() {
  const ab = 1;
  return ab;
}
foo();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
