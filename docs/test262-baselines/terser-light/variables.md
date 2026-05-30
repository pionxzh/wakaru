# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/variable
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
| 178 | 130 | 48 | 4 | 0 | 126 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | negative | 48 |
| unsupported | node-vm-baseline | 1 |
| unsupported | swc-parse-async-ident | 3 |

## Failures

No Wakaru correctness failures.
