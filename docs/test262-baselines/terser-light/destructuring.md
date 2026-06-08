# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/expressions/assignment/dstr, test/language/statements/for-of/dstr, test/language/statements/variable/dstr
- limit: all
- pipeline: terser-light
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
| 1034 | 891 | 143 | 28 | 54 | 809 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 18 |
| rejected | transform-reject | 36 |
| skipped | negative | 143 |
| unsupported | sloppy-only-strict-ident | 26 |
| unsupported | swc-parse-async-ident | 2 |

## Failures

No Wakaru correctness failures.

