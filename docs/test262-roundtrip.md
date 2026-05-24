# Test262 Round-Trip

`scripts/correctness/test262-roundtrip.mjs` checks semantic preservation by
running a Test262 file in Node's `vm`, transforming it, decompiling it with
wakaru, and running the decompiled result through the same Test262 harness.

The runner is intentionally feature-scoped. Prefer `--preset` or focused
`--path` values over running the whole Test262 repository.

## Commands

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit 500
node scripts\correctness\test262-roundtrip.mjs --limit all --json target\test262-default.json
node scripts\correctness\test262-roundtrip.mjs --limit all --summary target\test262-default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline babel-env-terser --limit 100 --summary target\test262-classes-babel.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --json target\test262-classes.json
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

Babel is an input producer, not the correctness oracle. The Test262 harness
remains the oracle: the original source, produced source, and Wakaru output must
all pass the same test.

## Timeouts and Reruns

`--case-timeout-ms <n>` bounds each runnable test case. The default is 30000 ms.
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
- `swc-parse-static-init-await`
- `swc-parse-static-async-constructor-method`
- `swc-parse-yield-ident`
- `swc-array-binding-elision`
- `swc-print-class-extends-arrow-parens`
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
node scripts\correctness\test262-roundtrip.mjs --preset templates --limit all --summary docs\test262-baselines\templates.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --limit all --summary docs\test262-baselines\modules.md
```

Recorded on 2026-05-25:

| Slice | Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 2180 | 1646 | 534 | 34 | 127 | 1485 | 0 |
| classes | 8426 | 5063 | 3363 | 34 | 680 | 4349 | 0 |
| destructuring | 1034 | 891 | 143 | 28 | 45 | 809 | 9 |
| async-generators | 1707 | 666 | 1041 | 13 | 6 | 642 | 5 |
| templates | 84 | 67 | 17 | 2 | 1 | 64 | 0 |
| modules | 755 | 157 | 598 | 142 | 6 | 9 | 0 |

Treat baseline movement as follows:

- `failed` going down means correctness improved or a non-Wakaru blocker was
  classified.
- `failed` going up needs investigation.
- `unsupported`/`rejected` moving is acceptable only when the reason is
  understood and documented in the JSON/report.
