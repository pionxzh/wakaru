# Test262 Round-Trip Summary

## Options

- complete: true
- test262Revision: 05bb032907160d66c212589d345fa0e335e2738c
- harnessVersion: 2
- nodeMajor: 22
- producerVersion: 5.31.6
- producerConfigHash: 845e39e180dd998ad60988ef83becf636083a7110da4e297c266d72a770ac7ad
- paths: test/language/expressions/addition, test/language/expressions/subtraction, test/language/expressions/multiplication, test/language/expressions/division, test/language/expressions/modulus, test/language/expressions/exponentiation, test/language/expressions/bitwise-and, test/language/expressions/bitwise-or, test/language/expressions/bitwise-xor, test/language/expressions/bitwise-not, test/language/expressions/left-shift, test/language/expressions/right-shift, test/language/expressions/unsigned-right-shift, test/language/expressions/logical-not, test/language/expressions/unary-minus, test/language/expressions/unary-plus, test/language/expressions/typeof, test/language/expressions/void, test/language/expressions/delete, test/language/expressions/postfix-decrement, test/language/expressions/postfix-increment, test/language/expressions/prefix-decrement, test/language/expressions/prefix-increment, test/language/expressions/equals, test/language/expressions/does-not-equals, test/language/expressions/strict-equals, test/language/expressions/strict-does-not-equals, test/language/expressions/greater-than, test/language/expressions/greater-than-or-equal, test/language/expressions/less-than, test/language/expressions/less-than-or-equal, test/language/expressions/in, test/language/expressions/instanceof, test/language/expressions/relational, test/language/expressions/assignment, test/language/expressions/compound-assignment, test/language/expressions/logical-assignment
- limit: all
- pipeline: terser-light
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
| 2200 | 2200 | 0 | 108 | 83 | 2009 | 0 |

## Reasons

| Status | Reason | Count |
|---|---|---:|
| rejected | swc-array-binding-elision | 9 |
| rejected | transform-reject | 72 |
| rejected | transform-runtime | 2 |
| unsupported | node-vm-baseline | 93 |
| unsupported | sloppy-only-strict-ident | 14 |
| unsupported | swc-parse-async-ident | 1 |

## Failures

No Wakaru correctness failures.
