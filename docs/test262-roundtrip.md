# Test262 Round-Trip

`scripts/correctness/test262-roundtrip.mjs` checks semantic preservation by
running a Test262 file in Node's `vm`, transforming it, decompiling it with
wakaru, and running the decompiled result through the same Test262 harness.
Module tests are different: `--preset modules` follows static relative
imports/re-exports, writes a temporary ESM graph, and runs that graph in Node.

The runner is intentionally feature-scoped. Prefer `--preset` or focused
`--path` values over running the whole Test262 repository.

## Corpus setup

The default Test262 corpus is a managed, shallow checkout pinned by
`scripts/correctness/test262-upstreams.json`. It is stored under the ignored
`target/correctness-tools/test262/vendor/` directory rather than as a git
submodule.

```powershell
node scripts\correctness\test262-corpus.mjs setup
node scripts\correctness\test262-corpus.mjs status
node scripts\correctness\test262-corpus.mjs setup --offline
```

Setup refuses to modify a dirty checkout. `--force` explicitly replaces dirty
or mismatched fixture state. Updating the tracked revision is separate and does
not regenerate or classify baselines:

```powershell
node scripts\correctness\test262-corpus.mjs update --revision <full-commit-sha>
```

`--test262 <dir>` remains available for focused work with another checkout;
reports identify such a non-git fixture as `unmanaged`.

## Commands

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit 500
node scripts\correctness\test262-roundtrip.mjs --limit all --json target\test262-default.json
node scripts\correctness\test262-roundtrip.mjs --limit all --summary target\test262-default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline babel-env-terser --limit 100 --summary target\test262-classes-babel.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline swc-minify --limit 100 --summary target\test262-classes-swc.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline esbuild-minify --limit 100 --summary target\test262-classes-esbuild.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --json target\test262-classes.json
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline swc-minify --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-swc.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline esbuild-minify --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-esbuild.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline babel-env-terser --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-babel.md
node scripts\correctness\compare-test262-reports.mjs target\before.json target\after.json --details
node scripts\correctness\test262-roundtrip.mjs --rerun-from target\test262-default.json --rerun-status failed --json target\test262-default-rerun.json
```

Defaults:

- `--pipeline terser-light`
- legacy equivalent: `--transform terser --terser-profile light`
- `--terser-profile light`
- `--level minimal`
- `--known-blockers scripts/correctness/test262-known-blockers.json`
- default paths: coalesce, optional chaining, object expressions, array
  expressions, `for-of`, and `let`

Use `--summary <file>` when you want a stable Markdown report suitable for
reviewing baseline movement in git diffs. It records options, totals,
reason-count buckets, and current Wakaru failures without timestamps.

When `--json` or `--summary` is provided, the runner updates that file after
each processed test. Interrupted runs leave `complete: false` in the report, so
the last saved result is still inspectable.

## Pipelines

`--pipeline` selects the producer that creates the code Wakaru decompiles:

- `none`: run the original Test262 source through Wakaru.
- `terser-light`: current default; Terser prints/minifies without compression or
  mangling.
- `terser-full`: Terser with compression and top-level mangling.
- `babel-env-terser`: Babel `preset-env` targeting IE 11, then `terser-light`.
- `swc-minify`: SWC minifier with compression and mangling disabled.
- `esbuild-minify`: esbuild transform with syntax/whitespace minification and
  stable identifiers.

Babel is an input producer, not the correctness oracle. The Test262 harness
remains the oracle: the original source, produced source, and Wakaru output must
all pass the same test.

## Module Graphs

`--preset modules` runs Test262 files with `flags: [module]` as module graphs
instead of single script files. The runner:

- parses static relative `import` and `export ... from` specifiers;
- recursively collects the entry module and local module dependencies;
- transforms every module with the selected producer in ESM mode;
- decompiles every transformed module independently with Wakaru;
- executes the original, transformed, and decompiled graphs through a temporary
  Node ESM package.

This is still not bundle testing. No packer is involved, and Wakaru is not run
with `--unpack`. The current purpose is to catch correctness bugs caused by ESM
semantics across files: live bindings, namespace objects, cycles, TDZ, export
aliases, and top-level await.

## Timeouts and Reruns

`--case-timeout-ms <n>` bounds each runnable test case. The default is 5000 ms.
Timeouts are recorded as `rejected` with reason `case-timeout`, so they are
visible in JSON and Markdown reports without losing the whole run.

Use `--rerun-from <json>` to rerun paths selected from a previous report. By
default it reruns `failed` results. Add one or more `--rerun-status` values to
include `rejected` or `unsupported` results too:

```powershell
node scripts\correctness\test262-roundtrip.mjs --rerun-from target\before.json --rerun-status failed --rerun-status rejected --json target\rerun.json
```

## Status Buckets

- `passed`: original, transformed, and decompiled code all pass.
- `unsupported`: the local Node/vm/SWC parser setup cannot run this input.
- `rejected`: the transform/minifier or known SWC print fidelity issue blocks
  the case before it can be treated as a Wakaru semantic failure.
- `failed`: a current Wakaru correctness candidate.

Known non-Wakaru reasons currently classified:

- `node-vm-baseline`
- `transform-reject`
- `transform-runtime`
- `sloppy-only-strict-ident`
- `swc-parse-async-ident`
- `swc-parse-await-class-name`
- `swc-parse-deep-iife-stack-overflow`
- `swc-parse-static-init-await`
- `swc-parse-static-async-constructor-method`
- `swc-parse-yield-arrow-parameter`
- `swc-parse-yield-function-name`
- `swc-parse-yield-ident`
- `swc-parse-yield-label`
- `swc-array-binding-elision`
- `swc-print-class-extends-arrow-parens`
- `swc-print-export-default-function-expression`
- `swc-print-new-arrow-parens`
- `swc-print-static-constructor-method`

Most known non-Wakaru classifications live in
`scripts/correctness/test262-known-blockers.json`. Keep entries narrow: match the
status, phase, path shape, error text, and decompiled output shape when possible.
Do not add a manifest entry for a real Wakaru semantic failure; fix the rule or
record it as a `failed` baseline instead.

## Baselines

Tracked baseline summaries live in `docs/test262-baselines/`. Normal summaries
are grouped by producer pipeline, and each producer runs the same slice set.
Regenerate the full normal matrix with:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs
```

Baseline paths encode two separate dimensions:

- the Test262 slice (`default`, `classes`, `modules`, and so on);
- the producer pipeline that transforms source before Wakaru runs
  (`terser-light`, `swc-minify`, `esbuild-minify`, and so on).

For example, `docs/test262-baselines/terser-light/default.md` means "default
Test262 slice through `terser-light`", not raw Test262 source. Use
`--pipeline none` or `--transform none` for a no-producer run. `terser-light`
uses Terser as a parser/printer with no compression or mangling.

Use `--producer` and `--slice` to refresh a subset:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs --producer swc-minify --slice operators
```

Add `--missing` to skip summaries that already exist and have `complete: true`.
The matrix runner builds `wakaru-cli` once before running jobs unless `WAKARU`
is already set.
The lower-level roundtrip runner does not fall back to `cargo run`; build the
CLI first or set `WAKARU` before calling it directly.

Both runners use parallel decompilation internally: the roundtrip runner batches
all wakaru invocations and runs them concurrently (bounded by `cpus - 2`), and
the matrix runner runs producer/slice jobs in parallel. A full 3-producer ×
20-slice matrix that previously took hours completes in minutes.

`swc-minify` and `esbuild-minify` are standalone producer pipelines; they are
not followed by Terser.

Module graph baselines live under `docs/test262-baselines/module-graph/`:
these run the modules slice with recursive local dependency loading. In this
directory, the file name is the producer pipeline.

```powershell
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline none --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\none.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline swc-minify --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\swc-minify.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline esbuild-minify --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\esbuild-minify.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline babel-env-terser --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\babel-env-terser.md
```

The recorded totals are **not** kept inline here — they live in two places
that stay current:

- `scripts/correctness/test262-stats.json` — cached totals per producer and
  slice (update with `test262-collect-stats.mjs`, verify with `--check`);
- [test262-baselines/](test262-baselines/) — the full deterministic Markdown
  summaries per producer/slice, reviewed as git diffs when baselines move.

Treat baseline movement as follows:

- `failed` going down means correctness improved or a non-Wakaru blocker was
  classified.
- `failed` going up needs investigation.
- `unsupported`/`rejected` moving is acceptable only when the reason is
  understood and documented in the JSON/report.

## Stats

`scripts/correctness/test262-stats.json` caches the current baseline totals so
other sessions can read them without regenerating all summaries. Update after
baseline changes:

```powershell
node scripts\correctness\test262-collect-stats.mjs                               # update all
node scripts\correctness\test262-collect-stats.mjs --producer swc-minify         # one producer
node scripts\correctness\test262-collect-stats.mjs --slice classes --slice scope  # specific slices
node scripts\correctness\test262-collect-stats.mjs --check                        # verify freshness
```

When `--producer` or `--slice` is given, only matching entries are re-collected;
the rest are kept from the existing stats file.
