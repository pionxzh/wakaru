# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/class, test/language/statements/class
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
| 8426 | 5064 | 3362 | 27 | 156 | 4880 | 1 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | case-timeout | 1 |
| rejected | transform-reject | 63 |
| rejected | transform-runtime | 92 |
| skipped | flag:async | 2086 |
| skipped | host-api | 10 |
| skipped | negative | 1266 |
| unsupported | node-vm-baseline | 24 |
| unsupported | swc-parse-async-ident | 3 |

## Failures

- test/language/expressions/class/elements/class-name-static-initializer-expr.js (decompiled-runtime)
