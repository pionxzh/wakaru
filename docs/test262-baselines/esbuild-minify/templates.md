# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/template-literal, test/language/expressions/tagged-template
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
| 84 | 67 | 17 | 2 | 1 | 64 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-runtime | 1 |
| skipped | host-api | 1 |
| skipped | negative | 16 |
| unsupported | node-vm-baseline | 2 |

## Failures

No Wakaru correctness failures.

