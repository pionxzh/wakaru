# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/assignmenttargettype
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
| 324 | 324 | 0 | 10 | 0 | 314 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| unsupported | node-parse-baseline | 8 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.
