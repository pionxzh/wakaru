# Wakaru

Wakaru is a JavaScript decompiler that transforms minified/bundled code back into readable, modern ESNext. It unpacks bundles (webpack4, webpack5, browserify, SystemJS, esbuild/Bun, AMD, plus heuristic scope-hoisted splitting), restores transpiler helpers (Babel, TypeScript), and applies ~60 rewrite rules to recover idiomatic source.

Written in Rust using the SWC AST ecosystem. The workspace is split into three crates under `crates/`: `core`, `cli`, and `wasm`.

## Understand the Project

Read these before contributing:
- `docs/architecture.md` — pipeline flow, components, design patterns
- `docs/testing.md` — test patterns, helpers, organization
- `docs/helper-detection.md` — how transpiler helpers are detected and restored
- `docs/debugging.md` — rule tracing, snapshot debugging, fixture workflow

## Building and Running

All `cargo` commands run from the repo root.

```bash
cargo build                                                 # debug build
cargo run -p wakaru-cli -- input.js -o output.js            # decompile single file
cargo run -p wakaru-cli -- --unpack bundle.js -o unpacked/  # unpack bundle
cargo run -p wakaru-cli -- --unpack --raw bundle.js -o raw/ # raw extraction (no rules)
cargo run -p wakaru-cli -- input.js -m input.js.map         # with source map
cargo run -p wakaru-cli -- debug trace path/to/module.js    # debug: per-rule diffs
```

## Testing

```bash
cargo nextest run -p wakaru-core               # full core suite (~25x faster than cargo test)
cargo nextest run --workspace                  # everything
cargo test -p wakaru-core --test my_rule_rule  # one test file
cargo test -p wakaru-core --test smart_inline_rule -- inline_single_use  # one test
cargo fmt --check                              # verify Rust formatting
cargo clippy -p wakaru-core --all-targets -- -D warnings  # lint core changes
```

The full suite runs much faster under [nextest](https://nexte.st) (one global
parallel pool vs. `cargo test`'s sequential per-binary runs). Install once with
`cargo install cargo-nextest --locked` (or `curl -LsSf https://get.nexte.st/latest/<os> | tar zxf - -C $HOME/.cargo/bin`).
`cargo test` still works for everything; nextest does not run doctests (there
are none today — use `cargo test --doc` if that changes).

Snapshot drift **fails** the test and writes a `.snap.new` (via `INSTA_UPDATE=new`
in `.cargo/config.toml`). Review the diff, then accept intentional changes with
`cargo insta accept` (or `INSTA_UPDATE=always cargo test` for a one-off bulk accept).
See `docs/testing.md` for test helpers, patterns, and organization.

## Developing a Rule

### Every change needs a unit test

**No code change is committed without a corresponding unit test.** Pipeline snapshot updates alone are not sufficient — they test the whole pipeline, not the individual change.

Write tests before implementation when the input→output is known:
1. Create `crates/core/tests/my_rule_rule.rs` with failing test cases
2. Implement `crates/core/src/rules/my_rule.rs` until tests pass
3. Run pipeline tests to check for regressions

For bugfixes to existing rules: add a regression test that reproduces the exact bug.

### Adding a new rule

1. Create `crates/core/tests/my_rule_rule.rs` with test cases (they will fail)
2. Create `crates/core/src/rules/my_rule.rs` implementing SWC's `VisitMut` trait
3. Add `mod my_rule;` and `pub use my_rule::MyRule;` in `crates/core/src/rules/mod.rs`
4. Add a `RuleDescriptor` for the rule at the right position in `crates/core/src/rules/pipeline.rs`
5. Run tests until all pass

### Where to place it in the pipeline

Rules run in a fixed order. Check `crates/core/src/rules/pipeline.rs` and place your rule where its dependencies are satisfied:
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
2. Run the full core suite (covers all pipeline + unpack snapshot tests):
   - `cargo nextest run -p wakaru-core`
3. Run formatting and lint checks:
   - `cargo fmt --check`
   - `cargo clippy -p wakaru-core --all-targets -- -D warnings` for core/rule changes
   - Use the relevant package or `cargo clippy --workspace --all-targets -- -D warnings` when touching other crates or shared workspace code
4. If snapshots change, inspect the diff — confirm the output is semantically better, not just different
5. `git status --short` — no stale `.snap.new` files or unrelated changes

## Important Rules

1. **All changes must be tested** — no exceptions.
2. **Always check `SyntaxContext`** — rules matching identifiers by name must guard on `unresolved_mark`.
3. **Use `BindingRenamer` for renames** — never rename by `sym` alone.
4. **Formatting must pass, but don't format opportunistically** — run `cargo fmt --check`; if formatting is needed, keep it limited to files you intentionally changed and avoid unrelated rustfmt churn.
5. **Inspect snapshot diffs** — "different" without "better" is a regression.
6. **Be honest about what works** — never overstate what was accomplished.

## Code Review Self-Check

- Before making a non-obvious choice, ask "why this and not the alternative?" Research until you can answer.
- If neighboring code does something differently, find out _why_ before deviating — its choices are often load-bearing.
- Don't take a bug report's suggested fix at face value; verify it's the right layer.
- Use `render_pipeline_until()` or `debug trace` to verify the AST shape reaching your rule.
