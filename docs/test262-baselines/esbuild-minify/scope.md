# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- nodeMajor: 22
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/statements/block, test/language/statements/const, test/language/statements/function, test/language/expressions/function, test/language/expressions/arrow-function, test/language/statements/with
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
| 1396 | 1146 | 250 | 10 | 190 | 946 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 6 |
| rejected | swc-print-new-arrow-parens | 1 |
| rejected | transform-reject | 1 |
| rejected | transform-runtime | 18 |
| rejected | transform-runtime-arrow-this | 3 |
| rejected | transform-runtime-inferred-name | 57 |
| rejected | transform-runtime-with-environment | 104 |
| skipped | negative | 250 |
| unsupported | node-vm-baseline | 10 |

## Failures

No Wakaru correctness failures.
