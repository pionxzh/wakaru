# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/expressions/class, test/language/statements/class
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
| 8426 | 8426 | 0 | 39 | 528 | 7859 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-print-class-extends-arrow-parens | 2 |
| rejected | swc-print-static-constructor-method | 2 |
| rejected | transform-reject | 7 |
| rejected | transform-reject-top-level-await | 8 |
| rejected | transform-runtime | 125 |
| rejected | transform-runtime-inferred-name | 384 |
| unsupported | node-vm-baseline | 35 |
| unsupported | swc-parse-static-async-constructor-method | 4 |

## Failures

No Wakaru correctness failures.
