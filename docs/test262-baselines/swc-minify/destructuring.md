# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/assignment/dstr, test/language/statements/for-of/dstr, test/language/statements/variable/dstr
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
| 1034 | 891 | 143 | 27 | 49 | 815 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 19 |
| rejected | transform-runtime | 30 |
| skipped | negative | 143 |
| unsupported | sloppy-only-strict-ident | 26 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.

