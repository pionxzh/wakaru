# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/identifiers
- limit: all
- pipeline: esbuild-minify
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 10000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 268 | 152 | 116 | 8 | 8 | 136 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 8 |
| skipped | negative | 116 |
| unsupported | node-vm-baseline | 8 |

## Failures

No Wakaru correctness failures.
