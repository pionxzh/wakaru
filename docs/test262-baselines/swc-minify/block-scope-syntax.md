# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/block-scope/syntax
- limit: all
- pipeline: swc-minify
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
| 113 | 11 | 102 | 0 | 0 | 11 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| skipped | negative | 102 |

## Failures

No Wakaru correctness failures.
