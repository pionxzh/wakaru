# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/statements/block, test/language/statements/const, test/language/statements/function, test/language/expressions/function, test/language/expressions/arrow-function, test/language/statements/with
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
| 1396 | 1396 | 0 | 19 | 8 | 1368 | 1 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 6 |
| rejected | swc-print-new-arrow-parens | 1 |
| rejected | transform-runtime | 1 |
| unsupported | node-vm-baseline | 10 |
| unsupported | sloppy-only-strict-ident | 3 |
| unsupported | swc-parse-async-ident | 3 |
| unsupported | swc-parse-static-init-await | 1 |
| unsupported | swc-parse-yield-arrow-parameter | 2 |

## Failures

- test/language/statements/const/fn-name-class.js (decompiled-runtime)
