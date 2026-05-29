# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/class, test/language/statements/class
- limit: all
- pipeline: swc-minify
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
| 8426 | 5064 | 3362 | 27 | 156 | 4881 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | case-timeout | 1 |
| rejected | transform-reject | 63 |
| rejected | transform-runtime | 92 |
| skipped | flag:async | 2086 |
| skipped | host-api | 10 |
| skipped | negative | 1266 |
| unsupported | node-vm-baseline | 24 |
| unsupported | swc-parse-async-ident | 3 |

## Failures

No Wakaru correctness failures.

