# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 1.7.26
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/module-code
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
| 755 | 599 | 156 | 39 | 1 | 559 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-runtime | 1 |
| skipped | fixture | 156 |
| unsupported | module-graph-baseline | 11 |
| unsupported | node-module-baseline | 26 |
| unsupported | node-vm-baseline | 1 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
