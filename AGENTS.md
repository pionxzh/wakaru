# Wakaru

Wakaru is a JavaScript decompiler that transforms minified/bundled code back into readable, modern ESNext. It unpacks bundles (webpack4, webpack5, esbuild, browserify), restores transpiler helpers (Babel, TypeScript), and applies ~60 rewrite rules to recover idiomatic source.

Written in Rust using the SWC AST ecosystem. The crate lives in `wakaru-rs/`.

## Understand the Project

Read these before contributing:
- `wakaru-rs/docs/architecture.md` — pipeline flow, components, design patterns
- `wakaru-rs/docs/testing.md` — test patterns, helpers, organization
- `wakaru-rs/docs/helper-detection.md` — how transpiler helpers are detected and restored
- `wakaru-rs/docs/debugging.md` — rule tracing, snapshot debugging, fixture workflow

## Building and Running

All `cargo` commands run from `wakaru-rs/`.

```bash
cargo build                                          # debug build
cargo run -- input.js -o output.js                   # decompile single file
cargo run -- --unpack bundle.js -o unpacked/          # unpack bundle
cargo run -- --unpack --raw bundle.js -o raw/         # raw extraction (no rules)
cargo run -- input.js -m input.js.map                 # with source map
cargo run -- --trace-rules path/to/module.js          # debug: per-rule diffs
```

## Testing

```bash
cargo test                                # all tests
cargo test --test my_rule_rule            # one test file
cargo test --test smart_inline_rule -- inline_single_use  # one test
INSTA_UPDATE=always cargo test            # update snapshots
cargo insta review                        # review snapshot diffs
```

See `docs/testing.md` for test helpers, patterns, and organization.

## Developing a Rule

### Every change needs a unit test

**No code change is committed without a corresponding unit test.** Pipeline snapshot updates alone are not sufficient — they test the whole pipeline, not the individual change.

Write tests before implementation when the input→output is known:
1. Create `tests/my_rule_rule.rs` with failing test cases
2. Implement `src/rules/my_rule.rs` until tests pass
3. Run pipeline tests to check for regressions

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

If your rule matches identifiers by name, you **must** check `SyntaxContext` to avoid matching the wrong binding:

```rust
if id.ctxt.outer() != self.unresolved_mark {
    return;
}
```

Every new visitor that matches identifiers by name must take `unresolved_mark: Mark` and gate on it. See `docs/architecture.md` for details.

### Renaming identifiers

Always use `rename_utils::BindingRenamer` (via `rename_bindings_in_module` or `rename_bindings`). Never write a custom `VisitMut` that renames by `sym` alone — it will hit inner-scope locals and parameters with the same name.

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

## Important Rules

1. **All changes must be tested** — no exceptions.
2. **Always check `SyntaxContext`** — rules matching identifiers by name must guard on `unresolved_mark`.
3. **Use `BindingRenamer` for renames** — never rename by `sym` alone.
4. **Don't format opportunistically** — `cargo fmt` on existing files creates unreviewable diffs. Only format in dedicated commits.
5. **Inspect snapshot diffs** — "different" without "better" is a regression.
6. **Be honest about what works** — never overstate what was accomplished.

## Code Review Self-Check

- Before making a non-obvious choice, ask "why this and not the alternative?" Research until you can answer.
- If neighboring code does something differently, find out _why_ before deviating — its choices are often load-bearing.
- Don't take a bug report's suggested fix at face value; verify it's the right layer.
- Use `render_pipeline_until()` or `--trace-rules` to verify the AST shape reaching your rule.
