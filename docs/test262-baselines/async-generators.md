# Test262 Round-Trip Summary

## Options

- paths: test/language/expressions/async-arrow-function, test/language/expressions/async-function, test/language/expressions/async-generator, test/language/expressions/generators, test/language/statements/async-function, test/language/statements/async-generator, test/language/statements/generators
- limit: all
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 1707 | 666 | 1041 | 13 | 6 | 642 | 5 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 3 |
| rejected | transform-runtime | 3 |
| skipped | flag:async | 756 |
| skipped | host-api | 2 |
| skipped | negative | 283 |
| unsupported | node-vm-baseline | 4 |
| unsupported | sloppy-only-strict-ident | 6 |
| unsupported | swc-parse-async-ident | 1 |
| unsupported | swc-parse-yield-ident | 2 |

## Failures

- test/language/expressions/generators/static-init-await-binding.js (wakaru)
- test/language/expressions/generators/unscopables-with-in-nested-fn.js (decompiled-runtime)
- test/language/expressions/generators/unscopables-with.js (decompiled-runtime)
- test/language/statements/generators/unscopables-with-in-nested-fn.js (decompiled-runtime)
- test/language/statements/generators/unscopables-with.js (decompiled-runtime)

