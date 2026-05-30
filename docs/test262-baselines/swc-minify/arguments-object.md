# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/arguments-object
- limit: all
- pipeline: swc-minify
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 10000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 263 | 202 | 61 | 0 | 0 | 202 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | flag:async | 60 |
| skipped | negative | 1 |

## Failures

No Wakaru correctness failures.
