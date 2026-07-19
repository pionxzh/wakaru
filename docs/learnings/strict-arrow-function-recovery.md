# Learning: strict positive-proof arrow recovery is too conservative for Standard

**TL;DR — Do not limit Standard-level `function` → arrow recovery to locally
call-only bindings and immediate invocations. That policy avoids having to
enumerate every possible constructor escape, but it also removes most useful
arrow recovery from exported functions and callbacks. The experiment caused 25
core-suite failures and reduced the reproduction-matrix baseline from
1778/1826 (97.4%) to 1744/1826 (95.5%). Keep the broad generated-code heuristic
at Standard, with explicit hard guards for observable ordinary-function
semantics.**

## Why the stricter policy was attractive

An ordinary function and an arrow differ in observable ways even when the body
does not mention `this` or `arguments`: ordinary functions can be constructed,
have a `.prototype`, and can participate as constructors in `instanceof`,
`extends`, and `Reflect.construct`. A negative guard for every dangerous use can
look like a list that will grow forever.

The proposed alternative was positive proof:

- Standard converts only an immediate function callee or a binding whose every
  visible use is a direct call.
- Standard preserves callbacks, exported functions, and other escaping values.
- Aggressive retains the existing broad conversion.

This is locally principled, but it cannot recover an original arrow once a
compiler has lowered an arrow that does not use lexical `this`/`arguments` to an
otherwise ordinary anonymous function. There is no remaining AST marker that
distinguishes it from a source-level function expression.

## What was built and measured

The experiment implemented the level-aware policy above, added positive and
negative unit tests, and ran the entire core suite and reproduction matrices.

- Core suite: 3122 tests run, 3097 passed, **25 failed**.
- Fourteen failures were Bun/Webpack/ESM snapshot groups with broad readability
  regressions: exported helpers and callbacks stayed as block-bodied functions.
- The remaining failures were mostly direct expected-output assertions in
  `ArrowReturn`, sliced-parameter recovery, SmartInline, and bundle tests. One
  real structural coupling was found and prototyped away: Vue setup-render
  recovery assumed the returned render closure was already an arrow.
- Aggregate reproduction rate: **1778/1826 (97.4%) → 1744/1826 (95.5%)**.
- All 34 lost rows were in the async/await matrix:
  **402/415 (96.9%) → 368/415 (88.7%)**. They were recovered async-arrow inputs
  that the strict policy could only print as async ordinary functions.
- Closure Compiler execution equivalence remained 15/15, so the stricter policy
  did not improve that matrix beyond the explicit constructor guards.

The policy and the Vue generalization were reverted after measurement. The
fixture snapshots and matrix baseline were not updated.

## What to do instead

Keep Standard's broad arrow recovery as a generated-code assumption, but treat
observable ordinary-function requirements as hard blockers:

- lexical/function-only body semantics: `this`, `arguments`, `new.target`,
  direct `eval`, generators, duplicate parameters, and named function
  expressions;
- statically visible constructor semantics: `new`, `.prototype`, `instanceof`,
  class `extends`, and the target/newTarget positions of `Reflect.construct`;
- propagate those constructor requirements backward through simple binding,
  assignment, member-path, conditional/logical, sequence, and `.bind` value
  flows.

This is intentionally not a proof against arbitrary dynamic escape. A function
passed to unknown code, exported, accessed through a dynamic property name, or
reached through `eval` can still be treated as a constructor outside the visible
AST. Eliminating that residual assumption requires the strict policy and its
measured recovery loss.

When adding a blocker, add a reproduced semantic counterexample and extend the
central constructor-sensitive value analysis. Do not add callee-name allowlists
or one-off package checks. The relevant direct JavaScript operations are a
small semantic category; producer-specific spellings are the unbounded list to
avoid.

## When to reconsider

Revisit positive-proof recovery only as an explicit new policy mode, or if
producer metadata/source maps can reliably distinguish lowered arrows from
source ordinary functions. Do not silently change Standard again without
measuring the full suite, fixture snapshots, and reproduction matrices.
