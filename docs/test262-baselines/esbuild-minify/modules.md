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
- caseTimeoutMs: 5000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 755 | 157 | 598 | 141 | 1 | 15 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 1 |
| skipped | flag:async | 2 |
| skipped | flag:module | 391 |
| skipped | negative | 205 |
| unsupported | node-vm-baseline | 141 |

## Failures

No Wakaru correctness failures.

