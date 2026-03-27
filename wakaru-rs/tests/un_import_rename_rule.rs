mod common;

use common::{assert_eq_normalized, render};

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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
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
    let output = render(input);
    assert_eq_normalized(&output, expected);
}
