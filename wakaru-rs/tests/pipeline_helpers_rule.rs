mod common;
use common::{assert_eq_normalized, render, render_pipeline_between, render_pipeline_until};

// ============================================================
// render_pipeline_until tests
// ============================================================

#[test]
fn pipeline_until_stops_before_later_rules() {
    // `!0` is normalized to `true` by UnminifyBooleans.
    // VarDeclToLetConst converts `var` to `const`.
    // Stopping before VarDeclToLetConst should leave `var`.
    let input = r#"var x = !0;"#;
    let after_booleans = render_pipeline_until(input, "UnminifyBooleans");
    // UnminifyBooleans ran: !0 → true
    assert!(
        after_booleans.contains("true"),
        "UnminifyBooleans should have converted !0 to true, got: {}",
        after_booleans
    );
    // VarDeclToLetConst has NOT run yet: still `var`
    assert!(
        after_booleans.contains("var "),
        "VarDeclToLetConst should not have run yet, got: {}",
        after_booleans
    );
}

#[test]
fn pipeline_until_full_matches_render() {
    // Stopping at the last rule should produce the same output as the full pipeline.
    let input = r#"var x = !0;"#;
    let full = render(input);
    let until_last = render_pipeline_until(input, "UnReturn");
    assert_eq_normalized(&full, &until_last);
}

#[test]
fn pipeline_until_early_stage_preserves_later_patterns() {
    // `void 0` is converted by RemoveVoid. `typeof x == "string"` is converted by UnTypeofStrict.
    // If we stop after FlipComparisons (before RemoveVoid and before UnTypeofStrict),
    // then `void 0` should remain. UnTypeofStrict runs before FlipComparisons? No —
    // looking at pipeline: SimplifySequence, FlipComparisons, UnTypeofStrict...
    // So stopping at FlipComparisons means UnTypeofStrict has NOT run.
    let input = r#"const x = typeof y == "string";"#;
    let result = render_pipeline_until(input, "FlipComparisons");
    // FlipComparisons flips == to ==, but UnTypeofStrict hasn't run so == stays
    assert!(
        result.contains(r#"== "string""#),
        "UnTypeofStrict should not have run, got: {}",
        result
    );

    // Now stop after UnTypeofStrict — should be ===
    let result2 = render_pipeline_until(input, "UnTypeofStrict");
    assert!(
        result2.contains(r#"=== "string""#),
        "UnTypeofStrict should have run, got: {}",
        result2
    );
}

// ============================================================
// render_pipeline_between tests
// ============================================================

#[test]
fn pipeline_between_runs_only_specified_range() {
    // Start from VarDeclToLetConst through SmartRename.
    // Input has `var` which VarDeclToLetConst should convert,
    // but SimplifySequence/FlipComparisons/etc should NOT run
    // (they're before the range).
    let input = r#"var x = !0;"#;
    let result = render_pipeline_between(input, "VarDeclToLetConst", "SmartRename");
    // VarDeclToLetConst ran: var → const
    assert!(
        result.contains("const "),
        "VarDeclToLetConst should have run, got: {}",
        result
    );
    // UnminifyBooleans did NOT run: !0 stays as !0
    assert!(
        result.contains("!0"),
        "UnminifyBooleans should not have run (before range), got: {}",
        result
    );
}

#[test]
fn pipeline_between_single_rule() {
    // Range of one rule: start = stop
    let input = r#"var x = !0;"#;
    let result = render_pipeline_between(input, "UnminifyBooleans", "UnminifyBooleans");
    // UnminifyBooleans ran
    assert!(
        result.contains("true"),
        "UnminifyBooleans should have run, got: {}",
        result
    );
    // VarDeclToLetConst did NOT run
    assert!(
        result.contains("var "),
        "VarDeclToLetConst should not have run, got: {}",
        result
    );
}

// ============================================================
// rule_names test
// ============================================================

#[test]
fn rule_names_contains_key_rules() {
    let names = wakaru_rs::rule_names();
    assert!(names.contains(&"SimplifySequence"), "missing SimplifySequence");
    assert!(names.contains(&"SmartInline"), "missing SmartInline");
    assert!(names.contains(&"SmartRename"), "missing SmartRename");
    assert!(names.contains(&"UnReturn"), "missing UnReturn (last rule)");
    assert!(names.contains(&"UnIife2"), "missing UnIife2 (second pass)");
    assert!(
        names.contains(&"UnWebpackInterop2"),
        "missing UnWebpackInterop2 (second pass)"
    );
    // First element should be SimplifySequence
    assert_eq!(names[0], "SimplifySequence");
    // Last element should be UnReturn
    assert_eq!(names[names.len() - 1], "UnReturn");
}
