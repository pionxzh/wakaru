mod common;

use common::{assert_eq_normalized, render_pipeline};

fn apply(input: &str) -> String {
    render_pipeline(input)
}

#[test]
fn bare_require_to_import() {
    // require('side-effect') → import 'side-effect'
    let input = "require('side-effect');";
    let expected = r#"import "side-effect";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn default_require_to_import() {
    let input = "var foo = require('foo');";
    let expected = r#"import foo from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn destructure_require_to_named_import() {
    // var { a, b: c } = require('foo')
    // UnEsm produces: import { a, b as c } from "foo"
    // UnImportRename then renames the alias `c` back to the imported name `b`
    let input = "var { a, b: c } = require('foo');";
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn property_require_to_named_import() {
    // UnEsm produces: import { baz as foo } from "bar"
    // UnImportRename then renames `foo` to `baz` (the imported name)
    let input = "var foo = require('bar').baz;";
    let expected = r#"import { baz } from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn default_property_require() {
    let input = "var foo = require('bar').default;";
    let expected = r#"import foo from "bar";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn webpack_default_getter_collapses_to_import() {
    let input = r#"
var r = require('foo');
var o = () => r && r.__esModule ? r.default : r;
function load() {
  return o();
}
"#;
    let expected = r#"
import r from "foo";
function load() {
  return r;
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn merge_same_source_imports() {
    let input = r#"
var foo = require('foo');
var { bar } = require('foo');
"#;
    let expected = r#"import foo, { bar } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn multiple_defaults_separate_imports() {
    let input = r#"
var foo = require('foo');
var bar = require('foo');
"#;
    let expected = r#"
import foo from "foo";
import bar from "foo";
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default() {
    let input = "module.exports = 1;";
    let expected = "export default 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_named_const() {
    let input = "exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_named_same_ident() {
    let input = r#"
function foo() {}
exports.foo = foo;
"#;
    let expected = r#"
function foo() {}
export { foo };
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop() {
    let input = "exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn export_dedup_void_init() {
    // void 0 → undefined after RemoveVoid rule, but the un_esm rule runs and detects void expr
    let input = r#"
exports.foo = void 0;
exports.foo = 1;
"#;
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn non_top_level_require_unchanged() {
    // VarDeclToLetConst converts var to const since bar is never reassigned.
    let input = r#"
function fn() {
  var bar = require('bar');
}
"#;
    let expected = r#"
function fn() {
  const bar = require('bar');
}
"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn module_exports_default_with_prop() {
    let input = "module.exports.foo = 1;";
    let expected = "export const foo = 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_named_diff_ident() {
    // UnEsm produces: function bar() {} + export { bar as foo }
    // UnExportRename then renames `bar` → `foo` and promotes to `export function foo() {}`
    let input = r#"
function bar() {}
exports.foo = bar;
"#;
    let expected = r#"export function foo() {}"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn exports_default_prop_module_exports() {
    let input = "module.exports.default = 42;";
    let expected = "export default 42;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn void_only_export_removed() {
    let input = "exports.foo = void 0;";
    let expected = "";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn self_ref_pattern_removed() {
    let input = "module.exports.default = module.exports;";
    let expected = "";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn existing_import_absorbed() {
    let input = r#"
import { a } from 'foo';
var { b } = require('foo');
"#;
    let expected = r#"import { a, b } from "foo";"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn compound_assign_not_transformed() {
    // module.exports += 1 should NOT be transformed
    let input = "module.exports += 1;";
    let expected = "module.exports += 1;";
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn bracket_notation_module_not_transformed() {
    // module["exports"] = 1 should NOT be transformed
    let input = r#"module["exports"] = 1;"#;
    let expected = r#"module["exports"] = 1;"#;
    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

