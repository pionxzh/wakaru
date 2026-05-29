# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/coalesce, test/language/expressions/optional-chaining, test/language/expressions/object, test/language/expressions/array, test/language/statements/for-of, test/language/statements/let
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
| 2180 | 1647 | 533 | 30 | 100 | 1517 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 64 |
| rejected | transform-runtime | 36 |
| skipped | flag:async | 231 |
| skipped | negative | 302 |
| unsupported | node-vm-baseline | 3 |
| unsupported | sloppy-only-strict-ident | 20 |
| unsupported | swc-parse-async-ident | 5 |
| unsupported | swc-parse-yield-function-name | 1 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.

