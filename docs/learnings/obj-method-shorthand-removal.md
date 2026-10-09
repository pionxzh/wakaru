# Learning: don't rewrite object-literal functions to method shorthand

**TL;DR — Wakaru had an `ObjMethodShorthand` rule that rewrote
`{ key: function () {} }` to `{ key() {} }`. It was removed, not level-gated.
Do not reintroduce it, at any rewrite level, and do not add detectors to make
it safe: method shorthand drops `[[Construct]]` and `prototype`, whether a
property is ever constructed cannot be decided from one module, and the
rewrite only saves a few characters.**

## What the rule cost

An object-literal method cannot be called with `new` and has no `prototype`.
ES5 code constructs object properties routinely, and usually somewhere the
rule cannot see:

- class-system helpers copy the literal's properties onto a class or a
  namespace (`Base.extend({ init: function () {} })`, `Lib.mixin({ make })`),
  and code constructs them later (`new Word.init()`);
- a lowered class keeps its members in `_createClass` descriptors
  (`{ key: "create", value: function () {} }`) when class recovery declines,
  and compiled ES5 code may construct a static member that ES2015 source could
  not;
- another module, or another script sharing a global, does the `new`.

The rule was guarded by a blacklist: keep the function when the module visibly
constructs the matching member. Each new counterexample needed another link —
call results and receivers of the call that receives the literal, assignment
chains, a two-segment name guard, suffixes collected across a multi-module
unpack, a `constructor` key exception — and each guard still missed shapes a
reader could construct in a few lines (a descriptor key read from `key:`, a
destructured import, a construct use created by a later rule).

Positive evidence does not rescue it either. A literal's properties are
provably never constructed only when the object stays local and every use is
a method call; most literals are exported, returned, stored, or passed to a
helper.

## What replaced it

Nothing: object-literal function values stay function expressions. Class
recovery (`UnPrototypeClass`, `UnEs6Class`) reads both `get: function () {}`
and `get() {}` descriptors, so it did not depend on the rule.

## When to reconsider

Only with a way to prove that a property is never constructed that covers
ordinary code, not with another detector for a construct use.
