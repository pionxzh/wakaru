# Debugging

This document collects workflow notes for investigating rule and snapshot regressions.

See also: [Testing](testing.md) for test helpers and patterns,
[Rule dependency inventory](rule-dependency-inventory.md) for pipeline ordering
and confirmed dependency chains.

## Quick Reference

```bash
# Trace all rules on a single file (shows diffs for each rule that changes output)
cargo run -- debug trace path/to/module.js

# Trace a specific range of rules
cargo run -- debug trace path/to/module.js --from RemoveVoid --until UnEsm

# Run all tests
cargo test

# Run a specific test file
cargo test --test my_rule_rule

# Update all snapshots after an intentional change
INSTA_UPDATE=always cargo test

# Review snapshot diffs interactively
cargo insta review

# Run with backtrace (useful for infinite recursion / panics)
RUST_BACKTRACE=1 cargo test -- --nocapture
```

## Rule Trace

Use the rule trace CLI before manually bisecting with `apply_rules_between`.
It runs the normal single-file rule pipeline and prints the initial source
once, followed by a git-style unified diff for each rule that changes the
rendered code. Rules that ran but left the output unchanged (with `--all`)
show up as a single `=== RuleName (unchanged) ===` header.

```bash
cargo run -- debug trace path/to/module.js
```

Useful options:

```bash
# Include rules that ran but did not change rendered output
cargo run -- debug trace path/to/module.js --all

# Trace only a range of rules
cargo run -- debug trace path/to/module.js --from RemoveVoid --until UnEsm
```

Rule names are the names returned by `rule_names()`, for example
`RemoveVoid`, `UnIife`, `SmartInline`, or `UnReturn`.

`debug trace` is intentionally single-file only. Bundle decompile uses the
two-phase fact-system pipeline, so tracing a full bundle would be misleading.
For bundle regressions, trace the extracted raw module or reduce the issue to a
single-file reproduction.

## Snapshot Layers

Webpack4 has two snapshot layers:

- `webpack4_unpack__*.snap` — final decompiled output, after rules.
- `webpack4_unpack_raw__*.snap` — raw module output after webpack
  normalization, before rules.

When a snapshot changes unexpectedly, compare the raw and final snapshots for
the same module. If the raw snapshot is unchanged but the final snapshot moved,
the cause is in the rule pipeline. If raw output changed too, inspect the
unpacker or webpack normalization first.

## Common Symptoms

- **Unexpected variable names:** Check for a missing `unresolved_mark` guard or
  matching by `sym` instead of `(sym, SyntaxContext)`.
- **Too many snapshots changed:** An early pipeline rule is cascading. Use
  `debug trace` on a representative module and check early rules like
  `SimplifySequence`, `FlipComparisons`, and `RemoveVoid`.
- **Rule not firing:** Check the raw snapshot. Earlier passes may have changed
  the AST shape before your rule runs.
- **`cargo test` hangs:** Likely infinite recursion. Run with
  `RUST_BACKTRACE=1 cargo test -- --nocapture`.

## Using render_pipeline_until and render_pipeline_between

When `debug trace` points to a rule but you need to write a focused test or
narrow down which rule in a range is causing a regression, use the pipeline
helper functions from `crates/core/tests/common/mod.rs` (documented in
[testing.md](testing.md)):

- **`render_pipeline_until(source, "RuleName")`** -- runs the pipeline from the
  start through the named rule (inclusive), then emits. Use this to see the
  cumulative output at a specific point in the pipeline.

- **`render_pipeline_between(source, "Start", "Stop")`** -- runs only the rules
  from `Start` through `Stop` (inclusive). Use this to isolate a narrow range
  when you suspect one of several adjacent rules.

Example workflow for a regression:

1. Run `debug trace` to find which rule introduced the regression.
2. Write a test using `render_pipeline_until` to capture the output just before
   that rule, confirming the input is what you expect.
3. Use `render_pipeline_between` to run only the suspect rule (or a small range)
   and verify the regression in isolation.
4. If the issue is a pipeline ordering problem, consult
   [rule-dependency-inventory.md](rule-dependency-inventory.md) for confirmed
   dependency chains and known fragile orderings.

## Fixture Repo

A private fixture repo at `../wakaru-fixtures/` contains bundled demo apps and
real-world bundles for cross-bundler regression testing. After significant rule
changes, run `./run.sh` there and check `git diff` for regressions.

Always rebuild the release binary before running fixtures. `run.sh` invokes
`target/release/wakaru` by default; it does not rebuild it for you.

```bash
cargo build --release && (cd ../../wakaru-fixtures && ./run.sh)
```

Before trusting a fixture diff, sanity-check the binary timestamp against your
most recent code change.
