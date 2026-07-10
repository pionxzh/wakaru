# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/coalesce, test/language/expressions/optional-chaining, test/language/expressions/object, test/language/expressions/array, test/language/statements/for-of, test/language/statements/let
- limit: all
- pipeline: terser-light
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 15000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 2180 | 1647 | 533 | 37 | 125 | 1485 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 9 |
| rejected | transform-reject | 113 |
| rejected | transform-runtime | 3 |
| skipped | flag:async | 231 |
| skipped | negative | 302 |
| unsupported | node-module-baseline | 1 |
| unsupported | node-vm-baseline | 5 |
| unsupported | sloppy-only-strict-ident | 22 |
| unsupported | swc-parse-async-ident | 5 |
| unsupported | swc-parse-static-init-await | 3 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.
