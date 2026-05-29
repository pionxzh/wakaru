# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/block, test/language/statements/const, test/language/statements/function, test/language/expressions/function, test/language/expressions/arrow-function, test/language/statements/with
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
| 1396 | 1146 | 250 | 19 | 33 | 1094 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 1 |
| rejected | transform-runtime | 22 |
| rejected | transform-runtime-with-environment | 10 |
| skipped | negative | 250 |
| unsupported | node-vm-baseline | 10 |
| unsupported | sloppy-only-strict-ident | 3 |
| unsupported | swc-parse-async-ident | 3 |
| unsupported | swc-parse-deep-iife-stack-overflow | 1 |
| unsupported | swc-parse-yield-arrow-parameter | 2 |

## Failures

No Wakaru correctness failures.
