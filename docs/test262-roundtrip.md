# Test262 Round-Trip

`scripts/correctness/test262-roundtrip.mjs` checks semantic preservation by
running a Test262 file in Node's `vm`, transforming it, decompiling it with
wakaru, and running the decompiled result through the same Test262 harness.
Module tests are different: `--preset modules` follows static relative
imports/re-exports, writes a temporary ESM graph, and runs that graph in Node.

The runner is intentionally feature-scoped. Prefer `--preset` or focused
`--path` values over running the whole Test262 repository.

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

Tracked baseline summaries live in `docs/test262-baselines/`. Regenerate them
with:

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit all --summary docs\test262-baselines\default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --summary docs\test262-baselines\classes.md
node scripts\correctness\test262-roundtrip.mjs --preset destructuring --limit all --summary docs\test262-baselines\destructuring.md
node scripts\correctness\test262-roundtrip.mjs --preset async-generators --limit all --summary docs\test262-baselines\async-generators.md
node scripts\correctness\test262-roundtrip.mjs --preset scope --limit all --summary docs\test262-baselines\scope.md
node scripts\correctness\test262-roundtrip.mjs --preset control-flow --limit all --summary docs\test262-baselines\control-flow.md
node scripts\correctness\test262-roundtrip.mjs --preset calls --limit all --summary docs\test262-baselines\calls.md
node scripts\correctness\test262-roundtrip.mjs --preset templates --limit all --summary docs\test262-baselines\templates.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --limit all --summary docs\test262-baselines\modules.md
```

Producer-specific baselines live under `docs/test262-baselines/<pipeline>/`.
Regenerate them by adding `--pipeline <name>` and writing into that directory:

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\classes.md
node scripts\correctness\test262-roundtrip.mjs --preset destructuring --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\destructuring.md
node scripts\correctness\test262-roundtrip.mjs --preset async-generators --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\async-generators.md
node scripts\correctness\test262-roundtrip.mjs --preset templates --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\templates.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --limit all --pipeline swc-minify --summary docs\test262-baselines\swc-minify\modules.md

node scripts\correctness\test262-roundtrip.mjs --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\classes.md
node scripts\correctness\test262-roundtrip.mjs --preset destructuring --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\destructuring.md
node scripts\correctness\test262-roundtrip.mjs --preset async-generators --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\async-generators.md
node scripts\correctness\test262-roundtrip.mjs --preset templates --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\templates.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --limit all --pipeline esbuild-minify --summary docs\test262-baselines\esbuild-minify\modules.md
```

Module graph baselines live under `docs/test262-baselines/module-graph/`:

```powershell
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline none --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\none.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline swc-minify --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\swc-minify.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline esbuild-minify --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\esbuild-minify.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline babel-env-terser --limit all --case-timeout-ms 2000 --summary docs\test262-baselines\module-graph\babel-env-terser.md
```

Recorded on 2026-05-25, with the scope, control-flow, and calls slices added on 2026-05-29:

| Slice | Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 2180 | 1647 | 533 | 34 | 128 | 1485 | 0 |
| classes | 8426 | 5063 | 3363 | 34 | 680 | 4349 | 0 |
| destructuring | 1034 | 891 | 143 | 28 | 45 | 809 | 9 |
| async-generators | 1707 | 666 | 1041 | 13 | 6 | 642 | 5 |
| scope | 1396 | 1146 | 250 | 20 | 8 | 1118 | 0 |
| control-flow | 1117 | 782 | 335 | 24 | 10 | 748 | 0 |
| calls | 193 | 185 | 8 | 7 | 0 | 178 | 0 |
| templates | 84 | 67 | 17 | 2 | 1 | 64 | 0 |
| modules | 755 | 157 | 598 | 142 | 6 | 9 | 0 |

Additional producer baselines recorded on 2026-05-25:

| Pipeline | Slice | Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| swc-minify | default | 2180 | 1646 | 534 | 30 | 99 | 1517 | 0 |
| swc-minify | classes | 8426 | 5063 | 3363 | 27 | 155 | 4881 | 0 |
| swc-minify | destructuring | 1034 | 891 | 143 | 27 | 49 | 815 | 0 |
| swc-minify | async-generators | 1707 | 666 | 1041 | 17 | 19 | 630 | 0 |
| swc-minify | templates | 84 | 67 | 17 | 2 | 0 | 65 | 0 |
| swc-minify | modules | 755 | 157 | 598 | 142 | 2 | 13 | 0 |
| esbuild-minify | default | 2180 | 1646 | 534 | 4 | 110 | 1532 | 0 |
| esbuild-minify | classes | 8426 | 5063 | 3363 | 28 | 387 | 4648 | 0 |
| esbuild-minify | destructuring | 1034 | 891 | 143 | 0 | 84 | 807 | 0 |
| esbuild-minify | async-generators | 1707 | 666 | 1041 | 7 | 40 | 619 | 0 |
| esbuild-minify | templates | 84 | 67 | 17 | 2 | 1 | 64 | 0 |
| esbuild-minify | modules | 755 | 157 | 598 | 141 | 1 | 15 | 0 |

Module graph baselines recorded on 2026-05-25:

| Pipeline | Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|
| none | 755 | 358 | 397 | 10 | 1 | 347 | 0 |
| swc-minify | 755 | 358 | 397 | 10 | 1 | 347 | 0 |
| esbuild-minify | 755 | 358 | 397 | 9 | 244 | 105 | 0 |
| babel-env-terser | 755 | 358 | 397 | 10 | 39 | 309 | 0 |

Treat baseline movement as follows:

- `failed` going down means correctness improved or a non-Wakaru blocker was
  classified.
- `failed` going up needs investigation.
- `unsupported`/`rejected` moving is acceptable only when the reason is
  understood and documented in the JSON/report.
