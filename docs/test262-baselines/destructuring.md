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
| 1034 | 891 | 143 | 28 | 45 | 809 | 9 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 9 |
| rejected | transform-reject | 36 |
| skipped | negative | 143 |
| unsupported | sloppy-only-strict-ident | 26 |
| unsupported | swc-parse-async-ident | 2 |

## Failures

- test/language/expressions/assignment/dstr/array-elem-trlg-iter-elision-iter-abpt.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elem-trlg-iter-elision-iter-nrml-close-err.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elem-trlg-iter-elision-iter-nrml-close-skip.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elem-trlg-iter-elision-iter-nrml-close.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elision-iter-abpt.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elision-iter-nrml-close-err.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elision-iter-nrml-close-skip.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-elision-iter-nrml-close.js (decompiled-runtime)
- test/language/expressions/assignment/dstr/array-iteration.js (decompiled-runtime)
