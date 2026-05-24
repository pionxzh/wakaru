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
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --json target\test262-classes.json
node scripts\correctness\compare-test262-reports.mjs target\before.json target\after.json --details
```

Defaults:

- `--transform terser`
- `--terser-profile light`
- `--level minimal`
- default paths: coalesce, optional chaining, object expressions, array
  expressions, `for-of`, and `let`

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
- `swc-parse-static-init-await`
- `swc-array-binding-elision`

## Baselines

Recorded on 2026-05-24 with:

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit all --json target\test262-roundtrip-default-all-after-arrow-tdz-fixes.json
```

| Slice | Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---|---:|---:|---:|---:|---:|---:|---:|
| default | 2180 | 1646 | 534 | 33 | 127 | 1476 | 10 |
| classes | 8426 | 5063 | 3363 | 28 | 676 | 4292 | 67 |
| destructuring | 1034 | 891 | 143 | 28 | 45 | 809 | 9 |
| async-generators | 1707 | 666 | 1041 | 11 | 6 | 636 | 13 |
| templates | 84 | 67 | 17 | 2 | 1 | 64 | 0 |
| modules | 755 | 157 | 598 | 142 | 6 | 9 | 0 |

Treat baseline movement as follows:

- `failed` going down means correctness improved or a non-Wakaru blocker was
  classified.
- `failed` going up needs investigation.
- `unsupported`/`rejected` moving is acceptable only when the reason is
  understood and documented in the JSON/report.
