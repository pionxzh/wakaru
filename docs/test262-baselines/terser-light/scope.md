# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/block, test/language/statements/const, test/language/statements/function, test/language/expressions/function, test/language/expressions/arrow-function, test/language/statements/with
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
| 1396 | 1146 | 250 | 20 | 8 | 1118 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 6 |
| rejected | swc-print-new-arrow-parens | 1 |
| rejected | transform-runtime | 1 |
| skipped | negative | 250 |
| unsupported | node-vm-baseline | 10 |
| unsupported | sloppy-only-strict-ident | 3 |
| unsupported | swc-parse-async-ident | 3 |
| unsupported | swc-parse-deep-iife-stack-overflow | 1 |
| unsupported | swc-parse-static-init-await | 1 |
| unsupported | swc-parse-yield-arrow-parameter | 2 |

## Failures

No Wakaru correctness failures.

