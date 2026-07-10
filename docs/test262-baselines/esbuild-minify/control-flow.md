# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/statements/if, test/language/statements/switch, test/language/statements/try, test/language/statements/return, test/language/statements/throw, test/language/statements/break, test/language/statements/continue, test/language/statements/labeled, test/language/statements/for, test/language/statements/for-in, test/language/statements/while, test/language/statements/do-while, test/language/expressions/conditional, test/language/expressions/logical-and, test/language/expressions/logical-or, test/language/expressions/comma
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
| 1117 | 782 | 335 | 21 | 44 | 717 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 2 |
| rejected | transform-runtime | 10 |
| rejected | transform-runtime-inferred-name | 32 |
| skipped | negative | 335 |
| unsupported | node-vm-baseline | 21 |

## Failures

No Wakaru correctness failures.
