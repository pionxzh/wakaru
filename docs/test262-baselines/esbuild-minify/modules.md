# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 24
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/module-code
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
| 755 | 599 | 156 | 38 | 280 | 281 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 11 |
| rejected | transform-reject-string-export-name | 6 |
| rejected | transform-reject-top-level-await | 233 |
| rejected | transform-runtime | 22 |
| rejected | transform-runtime-module-default-name | 7 |
| rejected | transform-runtime-module-this | 1 |
| skipped | fixture | 156 |
| unsupported | module-graph-baseline | 11 |
| unsupported | node-module-baseline | 26 |
| unsupported | node-vm-baseline | 1 |

## Failures

No Wakaru correctness failures.
