# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/statements/if, test/language/statements/switch, test/language/statements/try, test/language/statements/return, test/language/statements/throw, test/language/statements/break, test/language/statements/continue, test/language/statements/labeled, test/language/statements/for, test/language/statements/for-in, test/language/statements/while, test/language/statements/do-while, test/language/expressions/conditional, test/language/expressions/logical-and, test/language/expressions/logical-or, test/language/expressions/comma
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
| 1117 | 1117 | 0 | 24 | 10 | 1083 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 4 |
| rejected | transform-runtime | 6 |
| unsupported | node-vm-baseline | 21 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-yield-label | 2 |

## Failures

No Wakaru correctness failures.
