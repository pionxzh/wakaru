# Wakaru

Wakaru is a JavaScript decompiler that transforms minified/bundled code back into readable, modern ESNext. It unpacks bundles (webpack4, webpack5, esbuild, browserify), restores transpiler helpers (Babel, TypeScript), and applies ~60 rewrite rules to recover idiomatic source.

Written in Rust using the SWC AST ecosystem.

## Understand the Project

The Rust crate lives in `wakaru-rs/`. Read these first:
- `wakaru-rs/docs/architecture.md` — pipeline flow, components, design patterns
- `wakaru-rs/docs/helper-detection.md` — how transpiler helpers are detected and restored
- `wakaru-rs/docs/debugging.md` — rule tracing, snapshot debugging, fixture workflow

## Developing a Rule

### Every change needs a unit test

**No code change is committed without a corresponding unit test.** Pipeline snapshot updates alone are not sufficient — they test the whole pipeline, not the individual change.

Write tests before implementation when the input→output is known:
1. Create `tests/my_rule_rule.rs` with failing test cases
2. Implement `src/rules/my_rule.rs` until tests pass
3. Run pipeline tests to check for regressions

When exploring an unknown AST pattern, spike first, then write tests before finalizing.

For bugfixes to existing rules: add a regression test that reproduces the exact bug.

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
- Needs alias var declarations intact? Place before `SmartInline` (it removes `var h = p`)
- Needs export specifiers to reference real bindings? Place after `SmartInline`

### Scope-aware identifier matching

If your rule matches identifiers by name, you **must** check `SyntaxContext` to avoid matching the wrong binding. See the `unresolved_mark` guard pattern in `docs/architecture.md`.

### Renaming identifiers

Always use `rename_utils::BindingRenamer` (via `rename_bindings_in_module` or `rename_bindings`). Never write a custom `VisitMut` that renames by `sym` alone — it will hit inner-scope locals and parameters with the same name. `BindingRenamer` matches on `(Atom, SyntaxContext)` and correctly skips property names, member access, and handles import specifier `as` clauses.

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

### Formatting

Do not run `rustfmt` or `cargo fmt` as part of normal targeted fixes. Several existing Rust files have formatting drift, so formatting them opportunistically creates large unrelated diffs that make behavior changes harder to review.

Only format Rust code when:
- the change is a dedicated format-only commit, or
- a small newly added/rewritten file can be formatted without pulling unrelated churn into the diff.

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

## Debugging Tips

- **Find which rule changed a single file:** Run `cargo run -- --trace-rules path/to/module.js`. Use `--trace-all`, `--trace-from`, and `--trace-until` for narrower inspection. This is single-file only; for bundle regressions, trace an extracted raw module.
- **Unexpected variable names:** Check for missing `unresolved_mark` guard or matching by `sym` instead of `(sym, SyntaxContext)`. Compare raw vs decompiled snapshots.
- **Too many snapshots changed:** An early pipeline rule is cascading. Check `SimplifySequence` or `FlipComparisons` first.
- **Rule not firing:** Check the raw snapshot — the AST shape may differ from expectations after earlier passes.
- **`cargo test` hangs:** Likely infinite recursion. Run with `RUST_BACKTRACE=1 cargo test -- --nocapture`.
