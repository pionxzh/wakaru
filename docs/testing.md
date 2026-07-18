# Testing

See also: [Debugging](debugging.md) for investigating test failures and
snapshot regressions, [Architecture](architecture.md) for pipeline stage
ordering.

## Running Tests

```bash
# Run the full suite — prefer nextest (one global parallel pool; ~25x faster
# than `cargo test`, which runs the 90+ test binaries sequentially)
cargo nextest run -p wakaru-core      # core suite
cargo nextest run --workspace         # everything

# cargo test still works for everything, and for single-file / single-test focus
cargo test --test my_rule_rule
cargo test --test smart_inline_rule -- inline_single_use
```

Install nextest once with `cargo install cargo-nextest --locked` (or the
prebuilt binary from <https://get.nexte.st>). CI runs `cargo nextest run
--workspace --profile ci` (see `.config/nextest.toml`). nextest does not run
doctests; there are none today, but CI keeps a `cargo test --doc` guard.

Snapshot drift fails the test and writes a `.snap.new` (via `INSTA_UPDATE=new`
in `.cargo/config.toml`); accept intentional changes with `cargo insta accept`.

For semantic round-trip coverage with Test262, see
[Test262 Round-Trip](test262-roundtrip.md).

## Required Verification Before Commit

For code changes, run the full relevant checklist before committing. Do not
count a snapshot update alone as coverage for a rule change; add or update the
focused rule regression test as well.

1. Focused regression test for the rule or behavior you touched:

   ```bash
   cargo test -p wakaru-core --test my_rule_rule
   ```

2. The full core suite — it covers the pipeline and unpack snapshot tests for
   every supported bundler format, producer variant, and dialect (webpack4/5,
   Vercel ncc, browserify, Cocos Creator 2.x, Closure ModuleManager, SystemJS,
   esbuild/Bun, Metro, AMD/UMD, Rollup/Vite, multi-file), which is broader than
   any hand-picked subset:

   ```bash
   cargo nextest run -p wakaru-core
   ```

3. Recovery-rate baseline, when you changed a rule that a reproduction matrix
   covers (see "Reproduction Matrices" below):

   ```bash
   node scripts/repro/collect-stats.mjs --check
   ```

   If rates deliberately moved, regenerate without `--check` and commit the
   `stats.json` diff with the change. CI re-verifies this only when
   `scripts/repro/**` itself changes (`.github/workflows/repro-stats.yml`),
   so rule changes rely on this local step.

4. Formatting and linting:

   ```bash
   cargo fmt --check
   cargo clippy -p wakaru-core --all-targets -- -D warnings
   ```

   If you touched non-core crates or shared workspace code, run the matching
   package clippy command, or use:

   ```bash
   cargo clippy --workspace --all-targets -- -D warnings
   ```

5. Build the release-profile CLI only when you need a standalone binary, such
   as before running reproduction matrices with `WAKARU=target/dev-release/wakaru.exe`
   or when validating CLI/build behavior directly:

   ```bash
   cargo build --profile dev-release -p wakaru-cli
   ```

   The fixture runner below builds this profile itself, so do not run this as
   a separate required step only to prepare fixtures.

6. Fixtures, when the change can affect decompile output, unpacking, bundler
   behavior, rule ordering, helper detection, or CLI behavior. Run this only
   if you have the sibling `wakaru-fixtures` repository checked out. Run it from
   your worktree — it auto-detects and builds this checkout, decompiles every
   fixture into a scratch dir, and diffs against the committed reference without
   touching the working tree:

   ```bash
   ../wakaru-fixtures/run.sh --check     # exits non-zero on output drift
   ```

   On Windows, run the same script from Git Bash (it auto-detects `wakaru.exe`).
   To accept a deliberate, reviewed output improvement into the reference, run
   `../wakaru-fixtures/run.sh --update` and commit the `outputs/` change.

7. Docs freshness: if your change makes any statement in `docs/` or
   `AGENTS.md` false (a renamed flag, a moved file, a new rule ordering, a
   changed workflow), fix the doc in the same commit. Agents and new
   contributors trust the docs as reality — a wrong doc is worse than a
   missing one.

8. Final cleanliness checks:

   ```bash
   git diff --check
   git status --short
   ```

   `.cargo/config.toml` sets `INSTA_UPDATE=new`, so a changed snapshot **fails**
   the test and leaves a `.snap.new` instead of being silently accepted. Review
   each one, then accept intentional changes with `cargo insta accept` (or a
   one-off `INSTA_UPDATE=always cargo test`). Make sure no `.snap.new` files
   remain before committing.

Review every snapshot diff before committing. A snapshot change is acceptable
only when the output is semantically better or the test fixture expectation is
intentionally changing.

## Running Checks From a Worktree

All Cargo commands should be run from the wakaru worktree that contains the
changes you are validating, not from the main checkout by habit. The worktree
root is the directory that contains this repo's `Cargo.toml` and `docs/`
directory. (On Windows, use Git Bash so the same commands work.)

```bash
cd ../wakaru-my-worktree
cargo nextest run -p wakaru-core
cargo clippy -p wakaru-core --all-targets -- -D warnings
```

When a reproduction matrix needs a `WAKARU` binary, build and point to the
binary in the same worktree:

```bash
cd ../wakaru-my-worktree
cargo build --profile dev-release -p wakaru-cli
export WAKARU="$PWD/target/dev-release/wakaru"   # wakaru.exe on Windows
node scripts/repro/array-spread-rest-matrix/matrix.mjs --details
```

Without an explicit `WAKARU`, the shared matrix runner asks Cargo to refresh
the current worktree's debug CLI once before using `target/debug/wakaru`.

Do not reuse a `target/dev-release/wakaru` binary from another checkout unless
you are intentionally comparing against that checkout. A stale binary from
`main` can make a matrix pass or fail for the wrong code.

Fixtures are validated from the wakaru worktree under test. `run.sh`
auto-detects the worktree you launch it from and builds *that* checkout, so you
do not need to set `WAKARU` or worry about a stale binary:

```bash
cd ../wakaru-my-worktree
../wakaru-fixtures/run.sh --check
```

By default this writes to a scratch dir and diffs against the committed reference,
leaving both working trees clean — so it is safe to run from several worktrees
at once. It only modifies `wakaru-fixtures/outputs/` (and `report.txt`) when you
pass `--update` to accept a reviewed output improvement; commit that change in
the fixtures repo.

## Test Organization

All test files live under `crates/core/tests/`.

**Default: add your test to the existing test file for the rule you're changing.** Do not create a new file unless you're adding a new rule. Each rule has a corresponding test file.

- `*_rule.rs` -- Per-rule unit tests. One file per rule (e.g., `un_iife_rule.rs`, `smart_inline_rule.rs`).
- `noop_pipeline.rs` -- Stability tests: inputs that should pass through unchanged.
- `webpack4_unpack.rs` -- Pipeline snapshot tests for final webpack4 decompile output.
- `webpack4_unpack_raw.rs` -- Snapshot tests for webpack4 raw-unpack extraction, before the normal decompile pipeline.
- `bundle_unpack.rs` -- Pipeline snapshot tests for webpack5 + browserify bundles.
- `esbuild_unpack.rs` -- esbuild bundle detection and unpack tests.
- `systemjs_unpack.rs` -- SystemJS unpack tests using generated compiler and bundler fixtures.
- `closure_module_manager_unpack.rs` -- Closure ModuleManager/gstatic structural,
  guard, raw/full, multi-file, provenance, and pinned Closure Compiler-generated
  chunk tests.
- `metro_unpack.rs` -- Metro detection, extraction-boundary, provenance, and pipeline snapshots.
- `webpack5_chunk_unpack.rs` -- webpack5 chunk splitting tests.
- `webpack_fixtures.rs` -- Checked-in bundles generated by webpack4/5 and
  Vercel ncc, including runtime and wrapper variants.
- `cocos_creator_unpack.rs` -- Cocos Creator 2.x Browserify-family detection,
  dependency remapping, entry, provenance, and false-positive tests using a
  checked-in generated fixture.
- `multi_file_unpack.rs` -- Multi-input unpack tests for entry + chunk workflows.
- `facts_rule.rs` -- Cross-module fact extraction tests.
- `pipeline_helpers_rule.rs` -- Transpiler helper detection + restoration pipeline tests.
- `decompile_options_rule.rs` -- Tests for `DecompileOptions` configuration.
- `common/mod.rs` -- Shared test helpers (see below).
- `snapshots/` -- Insta snapshot files (auto-generated, committed).

### Generated bundler reproductions

When a bug depends on output from a real bundler, keep the reproduction under
`crates/core/tests/bundles/<bundler>-gen/`:

1. Commit the smallest readable source inputs under `src/`.
2. Add the generator command to `generate.sh`; pin the producer version when
   its output shape may vary between releases.
3. Commit the generated artifact under `dist/` so tests do not require the
   producer toolchain or network access.
4. Add focused coverage that reads the generated artifact. Do not replace it
   with a hand-written approximation of the reported bundle shape.

Generated artifacts should not be edited by hand. Regenerate them from the
checked-in inputs, and verify deterministic output before committing when the
producer permits it.

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
use wakaru_core::rules::UnDoubleNegation;

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

## Test Helpers (`crates/core/tests/common/mod.rs`)

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

## Snapshot Testing Workflow

Tests use [insta](https://insta.rs/) for snapshot testing. Snapshots are
committed as `.snap` files under `crates/core/tests/snapshots/`.

`.cargo/config.toml` sets `INSTA_UPDATE=new`, so a changed snapshot **fails** the
test and writes a `.snap.new` (it is not silently accepted). This keeps a
regression from landing green just because nobody eyeballed the `git diff`.

To review and accept intentional changes, install `cargo-insta` and run:

```bash
cargo insta review            # accept/reject each pending .snap.new
cargo insta accept            # accept all pending changes
INSTA_UPDATE=always cargo test  # one-off: bulk-accept inline during a run
```

**When snapshots change unexpectedly:** see the "Snapshot Layers" section in
[debugging.md](debugging.md) for how to trace the cause.

## Choosing `render` vs `render_rule`

`render_rule` runs a single rule in isolation (resolver + one rule + fixer).
`render` runs the full decompile pipeline. Most rule tests use `render_rule`,
but some rules depend on earlier normalization:

- **Helper-detection-dependent rules** (UnTemplateLiteral, UnAsyncAwait, etc.)
  rely on `LocalHelperContext::collect` scanning function bodies. Body-shape
  matchers may expect normalized forms — e.g. SimplifySequence splits comma
  returns into separate statements before tagged template body matching runs.
  If `render_rule` produces unchanged output but `render` works, the rule
  depends on earlier normalization. Use `render` for the test.

- **Rules after Stage 1 normalization** — if your test input has `void 0`,
  bracket notation, indirect calls, or comma expressions, those are normalized
  before your rule runs in the real pipeline. Either pre-normalize the test
  input manually, or use `render` / `render_pipeline_until`.

When in doubt, check with `debug trace` on the raw input to see what the AST
looks like when your rule receives it.

## Reproduction Matrices

Reproduction matrices under `scripts/repro/` test recovery across transpiler
versions, modes, and minification levels. Results are tracked in
`scripts/repro/stats.json` — read this file to see the current recovery rates
without re-running everything.

```bash
# Regenerate stats.json after rule/matrix changes
node scripts/repro/collect-stats.mjs

# Check whether stats.json matches a fresh run
node scripts/repro/collect-stats.mjs --check

# Run a single matrix with details
node scripts/repro/array-spread-rest-matrix/matrix.mjs --details

# Force a clean reinstall of matrix npm tools when investigating registry drift
WAKARU_REPRO_REFRESH_TOOLS=1 node scripts/repro/array-spread-rest-matrix/matrix.mjs --json

# Dump one shape for debugging
node scripts/repro/parameters-matrix/matrix.mjs --dump nested-default babel-7.8-loose
```

### Writing a new matrix

Every matrix should spread `...mangleValidator()` from `lib/compare.mjs` into
its `runMatrix()` config. This uses alpha-renaming normalization to compare
mangled shapes structurally rather than by substring needle matching — without
it, correctly-recovered mangled shapes show as false negatives.

```js
import { mangleValidator } from "../lib/compare.mjs";

runMatrix({
  name: "my-feature",
  snippets,
  transformers,
  ...mangleValidator(),
});
```

For snippets with legitimate structural variants in the output, add
`acceptForms` with the alternative full-program forms. For snippets where the
expected needle has multiple acceptable forms, use `expectedAny` (array of
needle-arrays — passes if any group is fully present).

### Execution equivalence (`execute`)

Substring and structural checks accept a *shape*; they cannot see a recovery
that assigns to a `const`, evaluates a receiver twice, or reads a stale key.
Rows that opt in with `execute` also run the lowered program and wakaru's
recovery in isolated `node:vm` contexts (`lib/exec-harness.mjs`) with identical
deterministic environments, and must produce the same effect log — otherwise
the row reports `behavior diverged` and counts as `no`.

```js
{
  name: "rest-computed-key",
  source: "...",
  expected: [...],
  execute: {
    env: { app_info: { size: 2, name: "app" } },  // JSON globals, cloned per run
    returns: { get_key: "size" },                  // stubs with fixed return values
  },
}
```

Every other free identifier is auto-stubbed as a deterministic recording
function, so sinks like `use(...)` need no declaration — their calls (with
deeply serialized arguments) *are* the observable behavior. Stub call order
and count are part of the log, which also pins single-evaluation contracts:
a recovery that calls `get_key()` twice diverges even if the result is right.
Shapes containing module syntax (e.g. `@swc/helpers` imports) skip the check
and keep their substring verdict, noted as `exec skipped` in the row.

Use `execute` on rows whose semantics have bitten before: declaration-kind
(mutated bindings), evaluation-count, evaluation-order, and enumeration-order
shapes. The check needs the debug binary to be current (`cargo build -p
wakaru-cli`) like every other matrix run.

## Test Pitfalls

- Don't use bare literal expression statements as test inputs (e.g. `65536;`) -- `SimplifySequence` drops them as dead code. Use `const x = 65536;` instead.
- When a test uses `render()` (full pipeline), other rules may transform the input before your rule runs. If your test fails unexpectedly, use `render_rule()` to isolate, or `render_pipeline_until()` to stop at a specific point. See [debugging.md](debugging.md) for more investigation workflows.
- `stmts_reference_ident` in `un_parameters.rs` matches by **emitted name** (ignoring SyntaxContext). A parameter named `e` will collide with any `e` in a nested function. This is intentional (prevents invalid parameter lists after rewriting) but can cause fold functions to bail out unexpectedly when an alias is inlined to a short parameter name.
