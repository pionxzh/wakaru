mod common;

use common::assert_eq_normalized;
use wakaru_rs::{decompile, DecompileOptions, RewriteLevel};

#[test]
fn dead_code_elimination_can_be_disabled() {
    let input = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            dead_code_elimination: false,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");

    let expected = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn dead_code_elimination_remains_enabled_by_default() {
    let input = r#"
import unused from "./x.js";
function helper() { return 1; }
export const value = 2;
"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
    )
    .expect("decompile should succeed");

    let expected = r#"
import "./x.js";
export const value = 2;
"#;
    assert_eq_normalized(&output, expected.trim());
}

#[test]
fn minimal_disables_loose_optional_chaining_recovery() {
    let input = r#"const x = U == null ? undefined : U.name;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Minimal,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");

    assert_eq_normalized(&output, input);
}

#[test]
fn standard_keeps_loose_optional_chaining_recovery_enabled() {
    let input = r#"const x = U == null ? undefined : U.name;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Standard,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");

    let expected = r#"const x = U?.name;"#;
    assert_eq_normalized(&output, expected);
}

#[test]
fn aggressive_enables_loose_optional_chaining_assignment_recovery() {
    let input = r#"const x = (n = e.ownerDocument) == null ? undefined : n.defaultView;"#;

    let output = decompile(
        input,
        DecompileOptions {
            filename: "fixture.js".to_string(),
            level: RewriteLevel::Aggressive,
            ..Default::default()
        },
    )
    .expect("decompile should succeed");

    let expected = r#"const x = e.ownerDocument?.defaultView;"#;
    assert_eq_normalized(&output, expected);
}
