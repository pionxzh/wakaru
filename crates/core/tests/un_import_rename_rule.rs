mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnImportRename;

fn apply(input: &str) -> String {
    render_rule(input, UnImportRename::new)
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
fn generates_unique_name_when_target_shadowed_by_nested_scope() {
    // `foo` is not declared at module scope, but a nested function binds it —
    // renaming the import to `foo` would silently shadow references to the
    // import inside that scope. Must pick `foo_1` instead.
    let input = r#"
import { foo as ab } from 'bar';
function outer() {
  const foo = 1;
  return ab();
}
ab();
"#;
    let expected = r#"
import { foo as foo_1 } from 'bar';
function outer() {
  const foo = 1;
  return foo_1();
}
foo_1();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generates_unique_name_when_target_shadowed_by_top_level_block() {
    // A `const foo` inside a module-level `if` block shadows references
    // to the renamed import within that block. The rename must account for
    // top-level block/catch scopes, not just function bodies.
    let input = r#"
import { foo as ab } from 'bar';
if (cond) {
  const foo = 1;
  ab();
}
ab();
"#;
    let expected = r#"
import { foo as foo_1 } from 'bar';
if (cond) {
  const foo = 1;
  foo_1();
}
foo_1();
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn generates_unique_name_when_target_shadowed_by_catch_binding() {
    // Same concern for `catch (foo)` at module scope: the catch binding
    // shadows the renamed import inside the handler body.
    let input = r#"
import { foo as ab } from 'bar';
try {
  doThing();
} catch (foo) {
  ab();
}
ab();
"#;
    let expected = r#"
import { foo as foo_1 } from 'bar';
try {
  doThing();
} catch (foo) {
  foo_1();
}
foo_1();
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

#[test]
fn keeps_alias_when_imported_name_is_reserved() {
    let input = r#"
import {
  if as if_,
  import as import_,
  let as let_,
  static as static_,
  await as await_,
  arguments as arguments_,
  eval as eval_
} from './module.js';
assert.sameValue(if_, 1);
assert.sameValue(import_, 2);
assert.sameValue(let_, 3);
assert.sameValue(static_, 4);
assert.sameValue(await_, 5);
assert.sameValue(arguments_, 6);
assert.sameValue(eval_, 7);
"#;
    let output = apply(input);
    assert_eq_normalized(&output, input);
}

#[test]
fn avoids_capturing_unresolved_reference_with_imported_name() {
    let input = r#"
import { A as A2 } from './self.js';
try {
  A;
} catch (error) {
  results.push(error.name);
}
export { A2 as B };
"#;
    let expected = r#"
import { A as A_1 } from './self.js';
try {
  A;
} catch (error) {
  results.push(error.name);
}
export { A_1 as B };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn nested_local_reference_does_not_block_top_level_import_rename() {
    let input = r#"
import { b as b_2 } from './module.js';
b_2("topLeft", "topRight");
function Component() {
  const b = useValue();
  return b;
}
"#;
    let expected = r#"
import { b } from './module.js';
b("topLeft", "topRight");
function Component() {
  const b = useValue();
  return b;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}
