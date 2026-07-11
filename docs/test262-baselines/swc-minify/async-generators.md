# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/await, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/for-await-of, test/language/statements/generators
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
| 2963 | 2963 | 0 | 43 | 71 | 2847 | 2 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 4 |
| rejected | transform-runtime | 67 |
| unsupported | node-vm-baseline | 10 |
| unsupported | sloppy-only-strict-ident | 25 |
| unsupported | swc-parse-async-ident | 4 |
| unsupported | swc-parse-yield-function-name | 2 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

- test/language/expressions/async-arrow-function/arrow-returns-promise.js (decompiled-runtime)
- test/language/statements/async-generator/return-undefined-implicit-and-explicit.js (decompiled-runtime)
