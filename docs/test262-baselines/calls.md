# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/call, test/language/expressions/new, test/language/expressions/member-expression, test/language/expressions/property-accessors, test/language/expressions/this, test/language/expressions/new.target
- limit: all
- pipeline: terser-light
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
| 193 | 185 | 8 | 7 | 0 | 178 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | flag:async | 1 |
| skipped | host-api | 2 |
| skipped | negative | 5 |
| unsupported | node-vm-baseline | 7 |

## Failures

No Wakaru correctness failures.

