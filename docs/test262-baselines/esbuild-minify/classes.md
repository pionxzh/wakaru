# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/class, test/language/statements/class
- limit: all
- pipeline: esbuild-minify
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
| 8426 | 5064 | 3362 | 28 | 389 | 4647 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | case-timeout | 1 |
| rejected | swc-print-class-extends-arrow-parens | 2 |
| rejected | swc-print-static-constructor-method | 2 |
| rejected | transform-reject | 7 |
| rejected | transform-runtime | 121 |
| rejected | transform-runtime-inferred-name | 256 |
| skipped | flag:async | 2086 |
| skipped | host-api | 10 |
| skipped | negative | 1266 |
| unsupported | node-vm-baseline | 24 |
| unsupported | swc-parse-static-async-constructor-method | 4 |

## Failures

No Wakaru correctness failures.

