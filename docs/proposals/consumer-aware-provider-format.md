# Provider format decisions that see their consumers

Status: proposed, not started; deferred by the maintainer (2026-10-15).
Unpack reports both failures below as warnings (`missing_imported_name`,
`commonjs_require_of_esm`) instead of fixing them. Revisit when another gap
needs a provider's format decision to see what its consumers do, or when
users report output that fails this way.

## Problem

Requiring a CommonJS module returns its `exports` object, and a consumer may
write to it:

```js
// provider
exports.f = function f() {};
// consumer
var p = require("./provider.js");
p.f = function patched() {};
```

The provider writes named exports only, so unpack recovers it as an ES
module (`export const f = …`). An ES module has no object a consumer can
write to: what a consumer gets is the module namespace, which is not
extensible and whose properties cannot be assigned from outside. The
consumer then fails in one of two ways:

- **It becomes ESM.** `UnEsm` gives it `import p from "./provider.js"`.
  Phase 2 turns such a default import into `import * as p` when the provider
  has no default export, but not when the binding is written, because a
  namespace cannot take the write. The default import stays, and the output
  fails to link (`does not provide an export named 'default'`) before any of
  the module graph that imports it runs, whether or not the write would
  ever execute.
- **It stays CommonJS** (it keeps another require). `require` of the
  provider now returns its namespace, so the write throws a TypeError in
  strict code and is lost in sloppy code.

In a large crawl of production sites, about one site in seven loads a bundle
whose unpack output breaks this way. Most of the writes come from a polyfill
library's internal modules, which replace a helper exported as `exports.f`;
others patch a utility module (`util.inherits = require("inherits")`).
Writes from code outside the input are a separate, accepted deviation
(`exports_object_closed_to_consumers` in
[rewrite-assumptions.md](../rewrite-assumptions.md#exports_object_closed_to_consumers)).

## Why Phase 2 cannot fix it

The fix is to keep a written provider CommonJS: then a default import gets
its `module.exports`, `require` returns the same object, and every write
lands where it did before. But the provider's format is decided by `UnEsm`
in Phase 1, where each module runs alone, before the barrier that makes
other modules' facts visible ([fact-system.md](../fact-system.md#shape)).
By the time a consumer's write is known, the provider is already ESM.

Phase 2 has no good repair on the provider side:

- There is no transform from ESM back to CommonJS.
- A facade object exported beside the named exports would take the write,
  but the provider's own code and every consumer that reads a named import
  would not see it.

Every earlier fix in this area repaired the consumer in Phase 2 (a kept
require reading `.default`, the passthrough redirect, the late interop
imports). This gap is the first that needs the provider's decision to
change.

## Options

### A. A scan barrier before `UnEsm`

Split Phase 1 at the point where the rule range would reach `UnEsm`. Before
the split, each module records the relative sources it writes through: a
member assignment, update, or `delete` on a `require("./p")` call or on a
binding declared from one, or `Object.assign`/`Object.defineProperty` with
one as the target. A barrier unions them into the set of written providers,
and `UnEsm` keeps each of those CommonJS.

- The scan reads the input after the normalizations that turn numeric ids
  into relative specifiers, but before rules reshape it, so it must accept
  the minified forms (`t(920).f = …`, comma sequences), as the existing
  pre-`UnEsm` fact collectors already do.
- Cost: every module waits at a second barrier before its rule range
  continues, so the slowest early-rule module delays all of them. The
  largest-first schedule limits but does not remove that.

### B. Re-run the written providers

Keep the single barrier. At the barrier, read each consumer's writes from
its post-`UnEsm` AST (a write through a default import or a kept require of
a relative source) and take the set of written providers. Run Phase 1 again
for only those providers, with `UnEsm` keeping them CommonJS, and replace
their facts before Phase 2 starts.

- The consumers need no change: `UnEsm` already gave them a default import,
  which is the right edge to a CommonJS provider once the provider facts say
  it stays CommonJS.
- Cost: a second, small parallel step, and each written provider's input
  kept or parsed again for it.
- A written provider that another provider re-exports needs that
  re-exporter's facts refreshed too.

B touches fewer modules and keeps the phase structure, so evaluate it first.

## Not in scope

- Consumers outside the input (the assumption above).
- A provider object that escapes into an unknown call that might write to
  it.

## Evidence to keep

- Artificial cases record both failure modes from producer builds.
- `missing_imported_name` and the `commonjs_require_of_esm` write reason
  count the affected modules on any corpus run, so a fix can be measured
  against them.
