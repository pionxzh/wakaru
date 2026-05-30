# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/keywords
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
| 25 | 0 | 25 | 0 | 0 | 0 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | negative | 25 |

## Failures

No Wakaru correctness failures.
