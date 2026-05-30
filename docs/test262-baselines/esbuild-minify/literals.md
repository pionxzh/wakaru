# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/literals
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
| 534 | 215 | 319 | 0 | 0 | 215 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | negative | 319 |

## Failures

No Wakaru correctness failures.
