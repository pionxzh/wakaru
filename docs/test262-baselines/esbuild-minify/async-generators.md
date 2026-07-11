# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 24
- producerVersion: 0.23.1
- producerConfigHash: 5cc7678984b5d2f567c58fa41b6aef47740bd1d8d00a72443d6b759eeeaf1a6f
- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/await, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/for-await-of, test/language/statements/generators
- limit: all
- pipeline: esbuild-minify
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json
- caseTimeoutMs: 15000
- rerunFrom: none
- rerunStatuses: none

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 2963 | 2963 | 0 | 13 | 229 | 2712 | 9 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 7 |
| rejected | transform-runtime | 52 |
| rejected | transform-runtime-inferred-name | 170 |
| unsupported | node-vm-baseline | 10 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

- test/language/expressions/async-arrow-function/arrow-returns-promise.js (decompiled-runtime)
- test/language/statements/async-generator/return-undefined-implicit-and-explicit.js (decompiled-runtime)
- test/language/statements/for-await-of/async-func-decl-dstr-array-elem-trlg-iter-elision-iter-nrml-close-null.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elem-trlg-iter-elision-iter-nrml-close-err.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elem-trlg-iter-elision-iter-nrml-close-null.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elem-trlg-iter-elision-iter-nrml-close-skip.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elem-trlg-iter-elision-iter-nrml-close.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elision-iter-nrml-close-skip.js (decompiled-runtime)
- test/language/statements/for-await-of/async-gen-decl-dstr-array-elision-iter-nrml-close.js (decompiled-runtime)
