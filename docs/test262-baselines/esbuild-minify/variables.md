# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/variable
- limit: all
- pipeline: esbuild-minify
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 5000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 178 | 130 | 48 | 1 | 17 | 112 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-runtime | 8 |
| rejected | transform-runtime-inferred-name | 9 |
| skipped | negative | 48 |
| unsupported | node-vm-baseline | 1 |

## Failures

No Wakaru correctness failures.
