# Testing

## Running Tests

```bash
# Run all tests
cargo test

# Run a specific test file
cargo test --test my_rule_rule

# Run a specific test within a file
cargo test --test smart_inline_rule -- inline_single_use

# Update snapshots after an intentional change
INSTA_UPDATE=always cargo test

# Review snapshot diffs interactively
cargo insta review
```

## Test Organization

**Default: add your test to the existing test file for the rule you're changing.** Do not create a new file unless you're adding a new rule. Each rule has a corresponding test file.

- `tests/*_rule.rs` — Per-rule unit tests. One file per rule (e.g., `un_iife_rule.rs`, `smart_inline_rule.rs`).
- `tests/noop_pipeline.rs` — Stability tests: inputs that should pass through unchanged.
- `tests/webpack4_unpack.rs` — Pipeline snapshot tests for webpack4 bundles (post-rules).
- `tests/webpack4_unpack_raw.rs` — Pipeline snapshot tests for webpack4 (pre-rules, after unpacker normalization).
- `tests/bundle_unpack.rs` — Pipeline snapshot tests for webpack5 + browserify bundles.
- `tests/esbuild_unpack.rs` — esbuild bundle detection and unpack tests.
- `tests/webpack5_chunk_unpack.rs` — webpack5 chunk splitting tests.
- `tests/facts_rule.rs` — Cross-module fact extraction tests.
- `tests/pipeline_helpers_rule.rs` — Transpiler helper detection + restoration pipeline tests.
- `tests/decompile_options_rule.rs` — Tests for `DecompileOptions` configuration.
- `tests/common/mod.rs` — Shared test helpers (see below).
- `tests/snapshots/` — Insta snapshot files (auto-generated, committed).

## Writing Tests

There are two test patterns: **full-pipeline tests** (run all rules) and **isolated rule tests** (run one rule only).

**Full-pipeline test** — use `render(input)`:

```rust
mod common;
use common::{assert_eq_normalized, render};

#[test]
fn my_feature_test() {
    let input = r#"void 0"#;
    let expected = r#"undefined"#;
    assert_eq_normalized(&render(input), expected);
}
```

**Isolated rule test** — use `render_rule(input, builder)`:

```rust
mod common;
use common::{assert_eq_normalized, render_rule};
use wakaru_rs::rules::UnDoubleNegation;

fn apply(input: &str) -> String {
    render_rule(input, |_| UnDoubleNegation)
}

#[test]
fn strips_double_bang_in_if() {
    let input = "if (!!x) { a(); }";
    let expected = "if (x) { a(); }";
    assert_eq_normalized(&apply(input), expected);
}
```

For rules that need `unresolved_mark`:

```rust
fn apply(input: &str) -> String {
    render_rule(input, |unresolved_mark| MyRule::new(unresolved_mark))
}
```

## Test Helpers (`tests/common/mod.rs`)

| Helper | Purpose |
|---|---|
| `render(source)` | Full decompile pipeline (all rules) |
| `render_rule(source, builder)` | Single rule in isolation (resolver + one rule + fixer) |
| `render_rule_with_filename(source, filename, builder)` | Same as `render_rule` but with custom filename (for `.ts`/`.tsx` parsing) |
| `render_pipeline_until(source, stop_after)` | Pipeline up to a specific rule (inclusive) |
| `render_pipeline_between(source, start, stop)` | Pipeline from `start` through `stop` (inclusive) |
| `trace_pipeline(source, options)` | Collect `RuleTraceEvent`s for debugging |
| `changed_rules(source)` | List which rule names changed the output |
| `normalize(input)` | Parse + re-emit to normalize whitespace |
| `assert_eq_normalized(actual, expected)` | Compare after normalizing both sides |

## Test Pitfalls

- Don't use bare literal expression statements as test inputs (e.g. `65536;`) — `SimplifySequence` drops them as dead code. Use `const x = 65536;` instead.
- When a test uses `render()` (full pipeline), other rules may transform the input before your rule runs. If your test fails unexpectedly, use `render_rule()` to isolate, or `render_pipeline_until()` to stop at a specific point.
