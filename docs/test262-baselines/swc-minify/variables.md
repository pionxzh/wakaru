# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/statements/variable
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
| 178 | 130 | 48 | 3 | 3 | 123 | 1 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-runtime | 3 |
| skipped | negative | 48 |
| unsupported | node-vm-baseline | 1 |
| unsupported | swc-parse-async-ident | 2 |

## Failures

- test/language/statements/variable/fn-name-class.js (decompiled-runtime)
