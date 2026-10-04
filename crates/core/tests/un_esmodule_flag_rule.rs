mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnEsmoduleFlag;

fn apply(input: &str) -> String {
    render_rule(input, UnEsmoduleFlag::new)
}

#[test]
fn removes_object_define_property_exports() {
    // Reused from packages/unminify/src/transformations/__tests__/un-esmodule-flag.spec.ts
    let input = r#"
Object.defineProperty(exports, '__esModule', { value: true });
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn removes_object_define_property_module_exports() {
    // Reused from packages/unminify/src/transformations/__tests__/un-esmodule-flag.spec.ts
    let input = r#"
Object.defineProperty(module.exports, '__esModule', { value: true });
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn removes_exports_esmodule_assign() {
    // Reused from packages/unminify/src/transformations/__tests__/un-esmodule-flag.spec.ts
    let input = r#"
exports.__esModule = !0;
exports.__esModule = true;
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn removes_module_exports_esmodule_assign() {
    // Reused from packages/unminify/src/transformations/__tests__/un-esmodule-flag.spec.ts
    let input = r#"
module.exports.__esModule = true;
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn removes_webpack_require_r_exports() {
    let input = r#"
require.r(exports);
require.r(module.exports);
"#;
    let expected = r#""#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_remove_unrelated_statements() {
    // Reused from packages/unminify/src/transformations/__tests__/un-esmodule-flag.spec.ts
    // UnEsmoduleFlag only removes __esModule=true (not false), and doesn't touch exports.foo
    // UnEsm converts exports.foo = 1 → export const foo = 1
    // exports.__esModule = false → export const __esModule = false (not removed by UnEsmoduleFlag)
    let input = r#"
exports.foo = 1;
Object.defineProperty(exports, 'foo', { value: 1 });
exports.__esModule = false;
"#;
    let expected = r#"
exports.foo = 1;
Object.defineProperty(exports, 'foo', { value: 1 });
exports.__esModule = false;
"#;

    let output = apply(input);
    assert_eq_normalized(&output, expected);
}

#[test]
fn does_not_remove_shadowed_exports_or_require() {
    let input = r#"
function outer(exports, require) {
  require.r(exports);
  exports.__esModule = true;
}
"#;

    let output = apply(input);
    assert_eq_normalized(&output, input);
}

// rollup 4.63 with `generatedCode.symbols`: the module marks itself with
// `Symbol.toStringTag`, alone or combined with `__esModule` when it has a
// default export. An ES module namespace has the same tag.
#[test]
fn removes_rollup_to_string_tag_markers() {
    let input = r#"
Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
Object.defineProperties(exports, { __esModule: { value: true }, [Symbol.toStringTag]: { value: 'Module' } });
exports.a = 1;
"#;
    let expected = r#"
exports.a = 1;
"#;
    assert_eq_normalized(&apply(input), expected);
}

#[test]
fn keeps_to_string_tag_definitions_that_are_not_the_marker() {
    let input = r#"
Object.defineProperty(exports, Symbol.toStringTag, { value: 'Custom' });
Object.defineProperty(exports, Symbol.toStringTag, { get: tag });
Object.defineProperties(exports, { __esModule: { value: true }, extra: { value: 1 } });
Object.defineProperty(other, Symbol.toStringTag, { value: 'Module' });
"#;
    assert_eq_normalized(&apply(input), input);
}

#[test]
fn shadowed_symbol_is_not_the_marker() {
    let input = r#"
const Symbol = { toStringTag: 'tag' };
Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
"#;
    assert_eq_normalized(&apply(input), input);
}
