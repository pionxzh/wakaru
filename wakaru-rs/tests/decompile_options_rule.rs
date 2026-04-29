mod common;

use common::assert_eq_normalized;
use wakaru_rs::{decompile, DecompileOptions};

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
