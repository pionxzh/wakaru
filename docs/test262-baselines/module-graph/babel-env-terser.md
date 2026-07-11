# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 24
- producerVersion: babel-7.25.2+preset-env-7.25.4+terser-5.31.6
- producerConfigHash: ddbcfc2b263a63a12dcae311e6d979865f5ee7cf27aa0762edfe1c3ae3ec8e92
- paths: test/language/module-code
- limit: all
- pipeline: babel-env-terser
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
| 755 | 599 | 156 | 39 | 46 | 514 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | transform-reject | 24 |
| rejected | transform-runtime | 19 |
| rejected | transform-runtime-module-default-name | 3 |
| skipped | fixture | 156 |
| unsupported | module-graph-baseline | 11 |
| unsupported | node-module-baseline | 26 |
| unsupported | node-vm-baseline | 1 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
