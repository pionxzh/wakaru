# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/block, test/language/statements/const, test/language/statements/function, test/language/expressions/function, test/language/expressions/arrow-function, test/language/statements/with
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
| 1396 | 1146 | 250 | 10 | 190 | 946 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 6 |
| rejected | swc-print-new-arrow-parens | 1 |
| rejected | transform-reject | 1 |
| rejected | transform-runtime | 182 |
| skipped | negative | 250 |
| unsupported | node-vm-baseline | 10 |

## Failures

No Wakaru correctness failures.
