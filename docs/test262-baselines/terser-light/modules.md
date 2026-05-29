# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/module-code
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
| 755 | 358 | 397 | 10 | 22 | 326 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 21 |
| rejected | transform-runtime | 1 |
| skipped | fixture | 156 |
| skipped | flag:async | 33 |
| skipped | host-api | 3 |
| skipped | negative | 205 |
| unsupported | module-graph-baseline | 8 |
| unsupported | node-module-baseline | 1 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
