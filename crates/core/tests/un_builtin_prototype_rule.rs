mod common;

use common::{assert_eq_normalized, render_rule, render_with_level};
use wakaru_core::rules::UnBuiltinPrototype;
use wakaru_core::RewriteLevel;

const SIX_LITERAL_RECEIVERS: &str = r#"
[].splice.apply(value, args);
"".indexOf.call(value, "x");
({}).hasOwnProperty.call(value, "x");
0..toFixed.call(value, 2);
/x/.test.call(value, "x");
(function() {}).call.apply(value, args);
"#;

const SIX_BUILTIN_PROTOTYPES: &str = r#"
Array.prototype.splice.apply(value, args);
String.prototype.indexOf.call(value, "x");
Object.prototype.hasOwnProperty.call(value, "x");
Number.prototype.toFixed.call(value, 2);
RegExp.prototype.test.call(value, "x");
Function.prototype.call.apply(value, args);
"#;

const SIX_STANDARD_LITERAL_RECEIVERS: &str = r#"
[].splice.apply(value, args);
"".indexOf.call(value, "x");
({}).hasOwnProperty.call(value, "x");
0..toFixed.call(value, 2);
/x/.test.call(value, "x");
(() => {}).call.apply(value, args);
"#;

fn apply(input: &str, level: RewriteLevel) -> String {
    render_rule(input, |_| UnBuiltinPrototype::new(level))
}

#[test]
fn aggressive_recovers_all_six_builtin_prototypes() {
    let output = apply(SIX_LITERAL_RECEIVERS, RewriteLevel::Aggressive);

    assert_eq_normalized(&output, SIX_BUILTIN_PROTOTYPES);
}

#[test]
fn minimal_preserves_literal_receivers() {
    let output = apply(SIX_LITERAL_RECEIVERS, RewriteLevel::Minimal);

    assert_eq_normalized(&output, SIX_LITERAL_RECEIVERS);
}

#[test]
fn standard_preserves_literal_receivers() {
    let output = apply(SIX_LITERAL_RECEIVERS, RewriteLevel::Standard);

    assert_eq_normalized(&output, SIX_LITERAL_RECEIVERS);
}

#[test]
fn full_pipeline_minimal_preserves_literal_receivers() {
    let output = render_with_level(SIX_LITERAL_RECEIVERS, RewriteLevel::Minimal);

    assert_eq_normalized(&output, SIX_LITERAL_RECEIVERS);
}

#[test]
fn full_pipeline_standard_preserves_literal_receivers() {
    let output = render_with_level(SIX_LITERAL_RECEIVERS, RewriteLevel::Standard);

    assert_eq_normalized(&output, SIX_STANDARD_LITERAL_RECEIVERS);
}

#[test]
fn full_pipeline_aggressive_recovers_all_six_builtin_prototypes() {
    let output = render_with_level(SIX_LITERAL_RECEIVERS, RewriteLevel::Aggressive);

    assert_eq_normalized(&output, SIX_BUILTIN_PROTOTYPES);
}
