# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/call, test/language/expressions/new, test/language/expressions/member-expression, test/language/expressions/property-accessors, test/language/expressions/this, test/language/expressions/new.target
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
| 193 | 185 | 8 | 7 | 0 | 178 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | flag:async | 1 |
| skipped | host-api | 2 |
| skipped | negative | 5 |
| unsupported | node-vm-baseline | 7 |

## Failures

No Wakaru correctness failures.
