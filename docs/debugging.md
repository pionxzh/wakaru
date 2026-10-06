# Debugging

This document collects workflow notes for investigating rule and snapshot regressions.

See also: [Testing](testing.md) for test helpers and patterns,
[Rule dependency inventory](rule-dependency-inventory.md) for pipeline ordering
and confirmed dependency chains, and [Debug and experimental CLI](cli-debug.md)
for the current `wakaru debug` interfaces and their compatibility policy.

## Quick Reference

```bash
# Trace all rules on a single file (shows diffs for each rule that changes output)
cargo run -p wakaru-cli -- debug trace path/to/module.js

# Trace a specific range of rules
cargo run -p wakaru-cli -- debug trace path/to/module.js --from RemoveVoid --until UnEsm

# Run all tests
cargo test

# Run a specific test file
cargo test --test my_rule_rule

# Run with backtrace (useful for infinite recursion / panics)
RUST_BACKTRACE=1 cargo test -- --nocapture

# Measure an emitted source map at token level (file or unpack directory)
node scripts/sourcemap/check.mjs input.js out.js

# Check identifier contexts after the pipeline over a directory of modules
cargo run --profile dev-opt -p wakaru-core --example name_capture_oracle -- path/to/modules/ > oracle.jsonl
```

## Rule Trace

The command signature and compatibility status are documented in
[cli-debug.md](cli-debug.md#debug-trace).

Use the rule trace CLI before manually bisecting with `apply_rules()` and
`RulePipelineOptions::between(...)`.
It runs the normal single-file rule pipeline and prints the initial source
once, followed by a git-style unified diff for each rule that changes the
rendered code. Rules that ran but left the output unchanged (with `--all`)
show up as a single `=== RuleName (unchanged) ===` header.

```bash
cargo run -p wakaru-cli -- debug trace path/to/module.js
```

Useful options:

```bash
# Include rules that ran but did not change rendered output
cargo run -p wakaru-cli -- debug trace path/to/module.js --all

# Trace only a range of rules
cargo run -p wakaru-cli -- debug trace path/to/module.js --from RemoveVoid --until UnEsm
```

Rule names are the names returned by `rule_names()`, for example
`RemoveVoid`, `UnIife`, `SmartInline`, or `UnReturn`.

`debug trace` is intentionally single-file only. Bundle decompile uses the
two-phase fact-system pipeline, so tracing a full bundle would be misleading.
For bundle regressions, trace the extracted raw module or reduce the issue to a
single-file reproduction. Keep in mind that trace treats the parsed file as a
source module: original ESM import reads and link checks are preserved, while
the unpack pipeline may remove unused imports that Wakaru recovered from bundle
edges.

## Validating Unpacked Output

The command signature, current output formats, and exit behavior are documented
in [cli-debug.md](cli-debug.md#debug-validate).

`debug validate` checks a directory of emitted modules as one graph and
reports structural findings that can indicate load-time or runtime failures:
dangling references, missing or ambiguous imported names, duplicate or conflicting
exports and declarations, leftover `module` / `exports` runtime uses, and
writes to imported or `const` bindings. The full finding inventory and the
source-goal rules live in [cli.md](cli.md). The command exits nonzero when
findings exist, so harnesses can gate on it. Const/import-write findings are
static: they do not prove that the assignment executes, that an error escapes
a catch, or that Wakaru introduced the write. Compare the original binding
and write when attributing a finding; `--input` only filters free-identifier
findings, not const/import writes.

```bash
cargo run -p wakaru-cli -- --unpack bundle.js -o out/
cargo run -p wakaru-cli -- debug validate out/          # human-readable
cargo run -p wakaru-cli -- debug validate out/ --json   # machine-readable
cargo run -p wakaru-cli -- debug validate out/ --input bundle.js  # + free identifiers not free in the input
```

Pass the original bundle with `--input` when triaging a real-world output: a
free identifier that the input never uses freely was introduced by wakaru,
while everything the input already left free (host probes, define constants,
upstream dependency bugs) stays silent. Without `--input`, free identifiers
are reported only when exactly one other emitted module declares the name at
module scope, the shape a split leaves when it drops an import/export edge.

Point it at **normal** unpack output only — `--raw` output promises only "no
readability transforms" and carries no module-graph contract, so raw-only
findings are not bugs. The checks are conservative: a provider whose export
set is unknowable (it re-exports an external package or a missing module)
suppresses missing-name findings for its consumers instead of guessing. The
implementation lives in `crates/core/src/output_validate.rs`.

## Output Source Maps

`scripts/sourcemap/check.mjs` measures an `--emit-source-map` result at token
level. It needs Node; the first run installs its pinned parser packages into the
shared repro tool cache (`docs/testing.md`).

```bash
wakaru input.js --emit-source-map -o out.js
node scripts/sourcemap/check.mjs input.js out.js

wakaru bundle.js --unpack --emit-source-map -o out/
node scripts/sourcemap/check.mjs bundle.js out/     # sums every module's map
```

It reports which output tokens carry a mapping at their start (overall and
by token kind), how many a lookup resolves, how many output positions carry
duplicate segments, and, for each mapped token, whether the input token at
the mapped position is the same token, an identifier under another name,
another token, the end of an input token, or none of these.

Reading the result:

- **No input token** points at a broken offset (a wrong line index, column
  unit, or composition hop). It should stay at or near zero.
- **Input token end** is closing punctuation mapped to the end of the node
  before it, such as the `)` in `f(a.b)` mapped to the end of `a.b` when the
  input puts the `)` on a later line. The emitter maps these from a node's
  end position, so they are expected. A position where one token ends and
  the next starts counts as a start.
- **Other token** is mostly faithful rewrites: `const <- var`,
  `true <- !`, an `if` mapped to the expression it replaced, a template
  mapped to a `.concat` call. Scan the listed pairs for ones that cannot be a
  rewrite of the mapped token.
- **Duplicate positions** should be zero; the map builders keep one mapping
  per output position.
- Punctuation and keywords such as `else`, `in`, and `instanceof` are
  written by the SWC emitter without a span, so their coverage is low by
  design. Empty template chunks and whitespace-only JSX text print nothing
  and are not counted.

`--last` resolves duplicate segments to the last one instead of the first,
`--json` prints the full result, and `--top` / `--examples` size the pair
list. The measures themselves live in `scripts/sourcemap/quality.mjs`; after
changing them, run `node --test scripts/sourcemap/quality.test.mjs`.

## Identifier Context Oracle

`crates/core/examples/name_capture_oracle.rs` runs parse → resolver → the
Standard pipeline over a directory of modules and reports identifier-context
defects that printed output cannot show:

- *captured*: an unresolved reference whose emitted name an enclosing scope
  declares, so the printed text binds to the local (a miscompile).
- *unmarked*: a reference with an empty `SyntaxContext`, which only a rule that
  built the identifier from a string can produce.
- *dangling*: a reference whose context is neither unresolved nor empty and
  matches no declared binding `(sym, ctxt)`; a rule minted a fresh context for
  a binding and rebuilt the reference or the declaration instead of cloning it.

```bash
cargo run --profile dev-opt -p wakaru-core --example name_capture_oracle -- \
  path/to/modules/ > oracle.jsonl        # one JSON object per module on stdout
                                         # aggregate totals on stderr
```

`ORACLE_ATTRIBUTE=1` re-runs the pipeline rule by rule for each module with a
residual and names the first rule after which it appears. The expected result
on any corpus is zero for all three. Run it after a change
to identifier synthesis, renaming, or the unpacker handoff; the fixture suite
compares text and stays green when only contexts are wrong. A dangling
reference also flags a rule that removed a declaration while an export
specifier or a reference to it survived, which prints as valid-looking text and
fails only when the module loads. The contract the oracle enforces is the
identity table in
[architecture.md](architecture.md#key-design-pattern-unresolved_mark).

## Snapshot Layers

Webpack4 has two snapshot layers:

- `webpack4_unpack__*.snap` — final decompiled output.
- `webpack4_unpack_raw__*.snap` — raw module output after webpack
  extraction and bundler-coupled normalization, before the normal decompile
  pipeline. Webpack `require.r(exports)` markers and `require.d(...)` getters
  can appear here; they are semantic inputs for later ESM recovery, not raw
  snapshot failures by themselves.

When a snapshot changes unexpectedly, compare the raw and final snapshots for
the same module. If the raw snapshot is unchanged but the final snapshot moved,
the cause is in the decompile pipeline. If raw output changed too, inspect the
unpacker or bundler-coupled normalization first.

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

Both names denote pipeline positions. A rule that is disabled at the requested
level or DCE mode still marks where the run starts or stops; it just does not
run itself, so stopping at a disabled rule never runs the rest of the pipeline.

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
changes, run `run.sh` from the worktree you are testing and check for drift.

```bash
cd ../wakaru-my-worktree
../wakaru-fixtures/run.sh --check       # build this worktree, diff vs reference
```

`run.sh` works on macOS, Linux, and Windows (via Git Bash — it auto-detects
`wakaru.exe`). It builds `wakaru-cli` with the `dev-opt` profile (optimized,
no LTO, incremental, so it rebuilds quickly after an edit; `--perf` runs use
`dev-release`) from the checkout you launch it in, so you never point at a
stale binary. By
default it diffs against the committed reference non-destructively; pass `--update`
to update the reference. Do not use a full `cargo build --release` fixture run
unless you specifically need release-LTO performance numbers.
