# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/module-code
- limit: all
- pipeline: esbuild-minify
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 2000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 755 | 358 | 397 | 9 | 244 | 105 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 218 |
| rejected | transform-runtime | 26 |
| skipped | fixture | 156 |
| skipped | flag:async | 33 |
| skipped | host-api | 3 |
| skipped | negative | 205 |
| unsupported | module-graph-baseline | 8 |
| unsupported | node-module-baseline | 1 |

## Failures

No Wakaru correctness failures.

