# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/generators
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
| 1707 | 666 | 1041 | 15 | 6 | 645 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 3 |
| rejected | transform-runtime | 3 |
| skipped | flag:async | 756 |
| skipped | host-api | 2 |
| skipped | negative | 283 |
| unsupported | node-vm-baseline | 5 |
| unsupported | sloppy-only-strict-ident | 6 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-static-init-await | 1 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

No Wakaru correctness failures.
