# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/coalesce, test/language/expressions/optional-chaining, test/language/expressions/object, test/language/expressions/array, test/language/statements/for-of, test/language/statements/let
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
| 2180 | 1647 | 533 | 4 | 111 | 1532 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 9 |
| rejected | transform-reject | 9 |
| rejected | transform-reject-top-level-await | 1 |
| rejected | transform-runtime | 18 |
| rejected | transform-runtime-inferred-name | 74 |
| skipped | flag:async | 231 |
| skipped | negative | 302 |
| unsupported | node-vm-baseline | 3 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.

