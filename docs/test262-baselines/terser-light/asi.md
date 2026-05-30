# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/asi
- limit: all
- pipeline: terser-light
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
| 102 | 66 | 36 | 0 | 3 | 63 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 3 |
| skipped | async-or-print | 1 |
| skipped | negative | 35 |

## Failures

No Wakaru correctness failures.
