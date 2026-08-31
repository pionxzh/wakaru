# Learning: don't rewrite `.then(null, handler)` to `.catch(handler)`

**TL;DR — Wakaru had an `UnThenCatch` rule that rewrote
`promise.then(null, handler)` / `.then(undefined, handler)` into
`.catch(handler)`. It was removed, not level-gated. Do not reintroduce it,
at any rewrite level: the pattern's main real-world occurrence is deliberate
`PromiseLike` support, where `.catch` does not exist and the rewrite turns
working code into a `TypeError`.**

## The rule and where it came from

`Promise.prototype.then(null, onRejected)` and
`Promise.prototype.catch(onRejected)` are semantically identical on a real
`Promise`, and `.catch()` reads better, so the rewrite looks like a free
readability win. The rule originated from observing the two-argument `.then`
shape in Sentry's published browser SDK bundles.

## Why it was removed instead of gated

Tracing the origin further showed the shape is very likely **intentional
source, not a compilation artifact**: TypeScript's `PromiseLike<T>` interface
guarantees only `.then()`. Library code written against `PromiseLike` (SDKs
accepting user-supplied thenables, A+ interop layers) uses
`.then(null, handler)` precisely because `.catch` may not exist on the
receiver.

That kills the rule at every level:

- At `standard`, the receiver's type is unknowable, so the rewrite was never
  sound there.
- Even as an `aggressive` producer-assumption rule (like
  `terser_unsafe_proto`), the assumption is backwards: aggressive rules assume
  the input shape is a transpiler/minifier artifact, but here the shape's
  natural habitat is hand-written `PromiseLike` interop — exactly the receiver
  where `.catch` is `undefined` and the output crashes.
- Adoption of the rewrite opportunity was near zero outside that habitat, so
  there is no recovery value to trade against the risk.

Removed in `fix(core): remove unsafe then-to-catch rewrite` together with a
`noop_pipeline.rs` test (`then_rejection_handler_without_promise_provenance_is_preserved`)
pinning that the pipeline leaves the shape alone.

## If the idea comes back

A sound version would need provenance that the receiver is a real `Promise`
(e.g. a fact-system proof that the expression is a native `Promise` literal or
`fetch(...)` result). Nothing short of that justifies the rewrite; "it is
usually a Promise" is exactly the assumption the `PromiseLike` contract
exists to break.
