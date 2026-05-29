# Test262 Round-Trip Summary

## Options

- complete: true
- paths: test/language/statements/if, test/language/statements/switch, test/language/statements/try, test/language/statements/return, test/language/statements/throw, test/language/statements/break, test/language/statements/continue, test/language/statements/labeled, test/language/statements/for, test/language/statements/for-in, test/language/statements/while, test/language/statements/do-while, test/language/expressions/conditional, test/language/expressions/logical-and, test/language/expressions/logical-or, test/language/expressions/comma
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
| 1117 | 782 | 335 | 24 | 14 | 744 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 2 |
| rejected | transform-runtime | 12 |
| skipped | negative | 335 |
| unsupported | node-vm-baseline | 21 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-yield-label | 2 |

## Failures

No Wakaru correctness failures.
