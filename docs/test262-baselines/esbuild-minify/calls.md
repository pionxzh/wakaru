# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/expressions/call, test/language/expressions/new, test/language/expressions/member-expression, test/language/expressions/property-accessors, test/language/expressions/this, test/language/expressions/new.target
- limit: all
- pipeline: esbuild-minify
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
| 193 | 193 | 0 | 9 | 3 | 181 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-runtime | 3 |
| unsupported | node-vm-baseline | 9 |

## Failures

No Wakaru correctness failures.
