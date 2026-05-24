# Test262 Round-Trip Summary

## Options

- paths: test/language/module-code
- limit: all
- pipeline: terser-light
- transform: terser
- terserProfile: light
- level: minimal
- knownBlockers: scripts/correctness/test262-known-blockers.json

## Totals

| Discovered | Runnable | Skipped | Unsupported | Rejected | Passed | Failed |
|---:|---:|---:|---:|---:|---:|---:|
| 755 | 157 | 598 | 142 | 6 | 9 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 6 |
| skipped | flag:async | 2 |
| skipped | flag:module | 391 |
| skipped | negative | 205 |
| unsupported | node-vm-baseline | 141 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
