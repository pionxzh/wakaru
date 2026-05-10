mod common;
use common::{
    assert_eq_normalized, changed_rules, render, render_pipeline_between, render_pipeline_until,
    trace_pipeline,
};
use wakaru_core::{
    format_trace_events, trace_rules, DecompileOptions, RuleTraceEvent, RuleTraceOptions,
};

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
    let names = wakaru_core::rule_names();
    assert!(
        names.contains(&"SimplifySequence"),
        "missing SimplifySequence"
    );
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

#[test]
fn trace_reports_changed_rules_only_by_default() {
    let events = trace_pipeline(
        "const x = void 0;",
        RuleTraceOptions {
            only_changed: true,
            ..Default::default()
        },
    );

    assert!(events.iter().all(|event| event.changed));
    assert!(
        events.iter().any(|event| event.rule == "RemoveVoid"),
        "expected RemoveVoid in changed trace, got: {:?}",
        events.iter().map(|event| event.rule).collect::<Vec<_>>()
    );
    assert!(
        events
            .iter()
            .any(|event| event.before.contains("void 0") && event.after.contains("undefined")),
        "expected before/after code around void replacement"
    );
}

#[test]
fn trace_can_include_unchanged_rules() {
    let events = trace_pipeline(
        "const x = 1;",
        RuleTraceOptions {
            stop_after: Some("FlipComparisons".to_string()),
            only_changed: false,
            ..Default::default()
        },
    );

    assert_eq!(
        events.iter().map(|event| event.rule).collect::<Vec<_>>(),
        vec!["SimplifySequence", "FlipComparisons"]
    );
    assert!(events.iter().any(|event| !event.changed));
}

#[test]
fn trace_supports_rule_ranges() {
    let events = trace_pipeline(
        "const x = void 0;",
        RuleTraceOptions {
            start_from: Some("RemoveVoid".to_string()),
            stop_after: Some("UnminifyBooleans".to_string()),
            only_changed: false,
        },
    );

    assert_eq!(
        events.iter().map(|event| event.rule).collect::<Vec<_>>(),
        vec!["RemoveVoid", "UnminifyBooleans"]
    );
}

#[test]
fn changed_rules_helper_returns_only_names() {
    let names = changed_rules("const x = void 0;");
    assert!(names.contains(&"RemoveVoid"));
}

// ============================================================
// format_trace_events tests
// ============================================================

fn event(rule: &'static str, before: &str, after: &str) -> RuleTraceEvent {
    RuleTraceEvent {
        rule,
        changed: before != after,
        before: before.to_string(),
        after: after.to_string(),
    }
}

#[test]
fn format_trace_prints_initial_source_once() {
    let events = vec![
        event("RuleA", "const x = 1;\n", "const x = 2;\n"),
        event("RuleB", "const x = 2;\n", "const x = 3;\n"),
    ];
    let output = format_trace_events(&events);

    // Exactly one "=== initial ===" block.
    assert_eq!(output.matches("=== initial ===").count(), 1);
    // The initial source line appears twice total: once in the initial block
    // and once as "-const x = 1;" inside RuleA's hunk — not three times.
    assert_eq!(output.matches("const x = 1;").count(), 2);
    // Intermediate state "const x = 2;" shows up only as diff body lines
    // (+ in RuleA, - in RuleB) — never as a full duplicated block.
    assert_eq!(output.matches("const x = 2;").count(), 2);
}

#[test]
fn format_trace_emits_unified_diff_for_changed_rules() {
    let events = vec![event(
        "RemoveVoid",
        "const x = void 0;\n",
        "const x = undefined;\n",
    )];
    let output = format_trace_events(&events);

    assert!(
        output.contains("=== RemoveVoid ===\n"),
        "missing rule header: {output}"
    );
    assert!(
        output.contains("@@"),
        "missing unified diff hunk header: {output}"
    );
    assert!(
        output.contains("-const x = void 0;"),
        "missing removed line: {output}"
    );
    assert!(
        output.contains("+const x = undefined;"),
        "missing added line: {output}"
    );
}

#[test]
fn format_trace_unchanged_rule_prints_only_header() {
    let events = vec![event("Noop", "const x = 1;\n", "const x = 1;\n")];
    let output = format_trace_events(&events);

    assert!(output.contains("=== Noop (unchanged) ===\n"), "{output}");
    assert!(
        !output.contains("@@"),
        "unchanged rule should not emit a diff hunk: {output}"
    );
    assert!(
        !output.contains("-const"),
        "unchanged rule should not emit removed lines: {output}"
    );
}

#[test]
fn format_trace_empty_returns_empty_string() {
    assert_eq!(format_trace_events(&[]), "");
}

#[test]
fn trace_rejects_unknown_rule_names() {
    let err = trace_rules(
        "const x = 1;",
        DecompileOptions {
            filename: "fixture.js".to_string(),
            ..Default::default()
        },
        RuleTraceOptions {
            stop_after: Some("NoSuchRule".to_string()),
            ..Default::default()
        },
    )
    .expect_err("unknown trace rule should fail");

    assert!(err.to_string().contains("NoSuchRule"));
}
