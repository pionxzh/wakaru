# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/template-literal, test/language/expressions/tagged-template
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
| 84 | 67 | 17 | 2 | 0 | 65 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | host-api | 1 |
| skipped | negative | 16 |
| unsupported | node-vm-baseline | 2 |

## Failures

No Wakaru correctness failures.
