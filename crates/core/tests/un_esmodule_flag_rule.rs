mod common;

use common::{assert_eq_normalized, render_rule};
use wakaru_core::rules::UnEsmoduleFlag;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnEsmoduleFlag)
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
