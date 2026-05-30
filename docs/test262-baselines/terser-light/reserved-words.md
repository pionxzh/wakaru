# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/reserved-words
- limit: all
- pipeline: terser-light
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
| 27 | 14 | 13 | 1 | 1 | 12 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | script-global-var-lexical-redeclaration | 1 |
| skipped | negative | 13 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
