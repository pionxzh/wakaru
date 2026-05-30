# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/assignmenttargettype
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
| 324 | 8 | 316 | 1 | 1 | 6 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 1 |
| skipped | negative | 316 |
| unsupported | swc-parse-yield-ident | 1 |

## Failures

No Wakaru correctness failures.
