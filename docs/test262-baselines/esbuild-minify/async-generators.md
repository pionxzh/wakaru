# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/generators
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
| 1707 | 666 | 1041 | 7 | 40 | 619 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 1 |
| rejected | transform-runtime | 39 |
| skipped | flag:async | 756 |
| skipped | host-api | 2 |
| skipped | negative | 283 |
| unsupported | node-vm-baseline | 5 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

No Wakaru correctness failures.

