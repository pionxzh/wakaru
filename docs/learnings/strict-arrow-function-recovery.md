# Learning: arrow recovery needs positive evidence, and async functions need none

**TL;DR — `ArrowFunction` converts a `function` expression only where positive
evidence shows the value never reaches `new`. A first attempt at this policy
was reverted because it lost every recovered async arrow; the adopted policy
adds the evidence that attempt lacked (async functions, callbacks whose
same-module callee only calls them, and a closed list of built-in callback
positions) and keeps the reproduction matrices unchanged. Do not go back to
converting by shape and blocking visible construct uses.**

## Why shape plus a blacklist failed

An ordinary function and an arrow differ observably even when the body does not
mention `this` or `arguments`: a function can be constructed and has a
`prototype`. The earlier rule converted by shape and blocked a growing list of
visible construct uses: `new`, `Reflect.construct`, `extends`, `instanceof`,
`.prototype`, a `createClass` helper's first argument, propagated backward
through aliases, IIFE returns, and parameters of same-module callees. Each
counterexample added another link, and each link still left the general case
open: a value passed to unknown code, exported, stored, or reached through a
dynamic property can be constructed where the module cannot see it. The same
happened to `ObjMethodShorthand`, which was removed
([obj-method-shorthand-removal.md](obj-method-shorthand-removal.md)).

## The first positive-proof attempt

The first attempt converted only an immediately invoked callee and a binding
whose every visible use is a direct call, at `standard`, and kept the broad
conversion at `aggressive`. It failed 25 core tests, mostly readability
snapshots, and lowered the reproduction-matrix aggregate from 1778/1826 to
1744/1826. All 34 lost rows were in the async/await matrix: recovered async
arrows printed as async ordinary functions. Vue setup-render recovery also
assumed the returned render closure was an arrow. The attempt was reverted.

## What the adopted policy adds

- **Async functions convert anywhere.** An async function has no
  `[[Construct]]` and no `prototype`, so the arrow changes nothing a caller can
  observe beyond what the existing `this`/`arguments` checks already cover.
  This alone covers the rows the first attempt lost.
- **Callbacks of a same-module callee that only calls the parameter.** The
  argument's value reaches only that parameter, and every use of the
  parameter is a call.
- **Built-in callback positions** (`builtin_callbacks_not_constructed` in
  [rewrite-assumptions.md](../rewrite-assumptions.md)): timer globals,
  `new Promise`, and array/Promise/string method names, at fixed argument
  positions. This is a name list, which the first write-up advised against.
  It is acceptable here because the list is closed and names language or host
  built-ins whose callbacks lowered code passes inline; it does not grow with
  library APIs.
- **Script scope.** A binding in a script's top-level scope is shared with
  other scripts and never counts as call-only; a module's top-level bindings
  and functions from an unwrapped top-level IIFE do.
- **Vue recovery** restores lowered arrows on its own analysis copy instead of
  depending on the pipeline's output ([vue-decompile.md](../vue-decompile.md)).

With these, the reproduction-matrix aggregate stayed at 2830/2991, including
the async/await matrix. What stays a function: member assignments
(`obj.x = function`), returned functions, object property values, escaping
bindings, and callbacks to library APIs. For ES5-era source those were
functions to begin with.

## When to reconsider

Add evidence, not blockers: a new proof that a value cannot reach `new`, or a
built-in callback position shown in lowered output. Do not reintroduce a
construct-use blacklist to win back conversions of values that escape.
