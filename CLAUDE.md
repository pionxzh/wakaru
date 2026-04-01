# Claude Code — Working on wakaru-rs

## Understand the Project

Read these first:
- `wakaru-rs/docs/architecture.md` — pipeline flow, components, design patterns
- `wakaru-rs/docs/helper-detection.md` — how transpiler helpers are detected and restored

## Developing a Rule

### Prefer test-first

Write tests before implementation when the input→output is known:
1. Create `tests/my_rule_rule.rs` with failing test cases
2. Implement `src/rules/my_rule.rs` until tests pass
3. Run pipeline tests to check for regressions

When exploring an unknown AST pattern, spike first, then write tests before finalizing.

### Adding a new rule

1. Create `tests/my_rule_rule.rs` with test cases (they will fail)
2. Create `src/rules/my_rule.rs` implementing SWC's `VisitMut` trait
3. Add `mod my_rule;` and `pub use my_rule::MyRule;` in `src/rules/mod.rs`
4. Add `module.visit_mut_with(&mut MyRule);` at the right position in `apply_default_rules()`
5. Run tests until all pass

### Where to place it in the pipeline

Rules run in a fixed order. Check `apply_default_rules()` in `src/rules/mod.rs` and place your rule where its dependencies are satisfied:
- Needs `["default"]` normalized to `.default`? Place after `UnBracketNotation`
- Needs `require()` calls present? Place before `UnEsm`
- Creates new IIFEs? Place before the second `UnIife` pass

### Scope-aware identifier matching

If your rule matches identifiers by name, you **must** check `SyntaxContext` to avoid matching the wrong binding. See the `unresolved_mark` guard pattern in `docs/architecture.md`.

## Writing Tests

Use `render(input)` for full-pipeline output:

```rust
mod common;
use common::{assert_eq_normalized, render};

#[test]
fn my_rule_test() {
    let input = r#"void 0"#;
    let expected = r#"undefined"#;
    assert_eq_normalized(&render(input), expected);
}
```

Don't use bare literal expression statements as test inputs (e.g. `65536;`) — `SimplifySequence` drops them as dead code. Use `const x = 65536;` instead.

## Testing Workflow

```bash
# Run all tests
cargo test

# Run a specific test file
cargo test --test my_rule_rule

# Update snapshots after an intentional change
INSTA_UPDATE=always cargo test

# Review snapshot diffs interactively
cargo insta review
```

### Two snapshot layers

- `webpack4_unpack__*.snap` — final decompiled output (what users see)
- `webpack4_unpack_raw__*.snap` — raw after webpack normalization, before rules

When debugging: diff raw vs decompiled snapshots for the same module to isolate which rule caused the change.

## Definition of Done

1. Run the focused rule tests you touched
2. Run pipeline tests:
   - `cargo test --test noop_pipeline`
   - `cargo test --test webpack4_unpack`
   - `cargo test --test webpack4_unpack_raw`
   - `cargo test --test bundle_unpack` (webpack5 + browserify)
   - `cargo test --test esbuild_unpack`
3. If snapshots change, inspect the diff — confirm the output is semantically better, not just different
4. `git status --short` — no stale `.snap.new` files or unrelated changes

## Fixture Repo

A private fixture repo at `../wakaru-fixtures/` contains bundled demo apps and real-world bundles for cross-bundler regression testing. After significant rule changes, run `./run.sh` there and check `git diff` for regressions. See that repo's README for details.

## Debugging Tips

- **Unexpected variable names:** Check for missing `unresolved_mark` guard or matching by `sym` instead of `(sym, SyntaxContext)`. Compare raw vs decompiled snapshots.
- **Too many snapshots changed:** An early pipeline rule is cascading. Check `SimplifySequence` or `FlipComparisons` first.
- **Rule not firing:** Check the raw snapshot — the AST shape may differ from expectations after earlier passes.
- **`cargo test` hangs:** Likely infinite recursion. Run with `RUST_BACKTRACE=1 cargo test -- --nocapture`.

## Review

OpenAI Codex will review your changes.
