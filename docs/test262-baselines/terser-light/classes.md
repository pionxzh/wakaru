# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/class, test/language/statements/class
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
| 8426 | 8426 | 0 | 45 | 992 | 7387 | 2 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-print-class-extends-arrow-parens | 2 |
| rejected | swc-print-static-constructor-method | 2 |
| rejected | transform-reject | 74 |
| rejected | transform-runtime | 914 |
| unsupported | node-vm-baseline | 35 |
| unsupported | swc-parse-async-ident | 4 |
| unsupported | swc-parse-await-class-name | 2 |
| unsupported | swc-parse-static-async-constructor-method | 4 |

## Failures

- test/language/expressions/class/elements/class-name-static-initializer-expr.js (decompiled-runtime)
- test/language/expressions/class/scope-name-lex-open-heritage.js (decompiled-runtime)
