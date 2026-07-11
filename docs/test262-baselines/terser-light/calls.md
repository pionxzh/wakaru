# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 24
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/call, test/language/expressions/new, test/language/expressions/member-expression, test/language/expressions/property-accessors, test/language/expressions/this, test/language/expressions/new.target
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
| 193 | 193 | 0 | 9 | 0 | 184 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| unsupported | node-vm-baseline | 9 |

## Failures

No Wakaru correctness failures.
