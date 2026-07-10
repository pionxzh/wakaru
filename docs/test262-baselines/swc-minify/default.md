# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/coalesce, test/language/expressions/optional-chaining, test/language/expressions/object, test/language/expressions/array, test/language/statements/for-of, test/language/statements/let
- limit: all
- pipeline: swc-minify
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
| 2180 | 1647 | 533 | 33 | 97 | 1517 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 61 |
| rejected | transform-runtime | 36 |
| skipped | flag:async | 231 |
| skipped | negative | 302 |
| unsupported | node-module-baseline | 1 |
| unsupported | node-vm-baseline | 5 |
| unsupported | sloppy-only-strict-ident | 20 |
| unsupported | swc-parse-async-ident | 5 |
| unsupported | swc-parse-yield-function-name | 1 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.
