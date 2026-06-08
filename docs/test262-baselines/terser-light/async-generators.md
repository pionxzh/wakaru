# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/await, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/for-await-of, test/language/statements/generators
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
| 2963 | 674 | 2289 | 21 | 7 | 646 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 3 |
| rejected | transform-runtime | 4 |
| skipped | flag:async | 1909 |
| skipped | host-api | 2 |
| skipped | negative | 378 |
| unsupported | node-vm-baseline | 5 |
| unsupported | sloppy-only-strict-ident | 6 |
| unsupported | swc-parse-async-ident | 7 |
| unsupported | swc-parse-static-init-await | 1 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

No Wakaru correctness failures.

