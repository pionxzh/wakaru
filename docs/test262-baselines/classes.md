# Test262 Round-Trip Summary

## Options

- paths: test/language/expressions/class, test/language/statements/class
- limit: all
- pipeline: terser-light
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 8426 | 5063 | 3363 | 34 | 680 | 4349 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-print-class-extends-arrow-parens | 2 |
| rejected | swc-print-static-constructor-method | 2 |
| rejected | transform-reject | 74 |
| rejected | transform-runtime | 602 |
| skipped | flag:async | 2078 |
| skipped | flag:module | 9 |
| skipped | host-api | 10 |
| skipped | negative | 1266 |
| unsupported | node-vm-baseline | 24 |
| unsupported | swc-parse-async-ident | 4 |
| unsupported | swc-parse-await-class-name | 2 |
| unsupported | swc-parse-static-async-constructor-method | 4 |

## Failures

No Wakaru correctness failures.
