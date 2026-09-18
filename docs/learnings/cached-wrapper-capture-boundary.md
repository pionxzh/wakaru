# Cached wrapper capture boundary

Keep property stores as capture exposures in `VarDeclToLetConst`, including
stores into the local exports object of a lazy CommonJS wrapper. This applies
at every rewrite level. Recognizing a cached-wrapper prologue does not prove
that the stored function waits for its captured bindings to initialize.

This decision is specific to declaration-kind recovery. It does not impose a
new purity requirement on unrelated recovery rules. The accepted proof and
its other limits remain in [rewrite assumptions](../rewrite-assumptions.md).

## Why the wrapper shape is insufficient

A tempting exception recognizes a cache guard, a fresh object and publication
of that object into an outer binding. It treats `out.read = read` as a link,
then exposes the linked function at later uses of the object. Three independent
problems prevent that syntax from proving delayed execution.

**Earlier object access can have lasting effects.** An alias retained before
the store can read the property afterward. A setter installed before the store
can execute the function during assignment. Exposing only functions already
stored at the earlier access misses both cases:

```js
var alias = out;
out.read = read;
console.log(alias.read());
var value = 42;
function read() { return value; }
```

Passing `out` to another function can retain the same alias. Installing a setter
on `out` or its prototype has the same timing problem. A fresh object at the
start is not proof that it remains unaliased or has no setters.

**Publication permits reentry.** A call need not mention the object or cache
inside the wrapper to expose its functions:

```js
var cache;
function factory(callback) {
  if (cache) return cache;
  var out = {};
  cache = out;
  out.read = read;
  callback();
  var value = 42;
  function read() { return value; }
  return out;
}
factory(() => console.log(factory().read()));
```

The callback obtains the published object by reentering the factory. The
original prints `undefined`; converting `value` to `const` or `let` introduces
a TDZ error. This is a local execution path, not an ESM-cycle policy question.

**Expressions can leave initialization unfinished.** Tracking explicit
`return` and `throw` statements does not cover exceptional completion:

```js
var cache;
function factory() {
  if (cache) return cache;
  var out = {};
  cache = out;
  out.read = read;
  JSON.parse("invalid");
  var value = 42;
  function read() { return value; }
  return out;
}
try { factory(); } catch {}
console.log(factory().read());
```

The original again prints `undefined`. After the first call fails, the cache
still exposes `read`, but a converted lexical binding would never initialize.
A scan reaching the declaration is not proof that execution reaches it.

## Chosen boundary

Keep the existing resolver-aware function/class reference graph and statement-
list initialization bounds. A function value stored in an arbitrary property
exposes its captures at the store. Do not add object-alias tracking, a wrapper
API model, control-flow analysis, or a delayed-property-use assumption at
`aggressive` to recover this shape.

This deliberately retains some safe `var` declarations. It also avoids adding
analysis cost or another family of syntax exceptions solely for readability.
The regression cases in `var_decl_to_let_const_rule.rs` cover retained aliases,
setters, callback reentry and exceptional completion at all levels, both in
isolation and through the pipeline.

A closed initialization prefix could be a different proof: every accepted
operation would need to exclude premature execution, escape, and exceptional
exit, with unknown operations rejecting the optimization. Side-effect-free is
not enough; even an identifier read can throw through a TDZ. A preliminary
screen found limited opportunities under that stricter boundary, so there is
no production implementation of it.

Revisit only with a reusable proof and measured material benefit. The first
experiment should establish both on a fixed pipeline input, not add another
wrapper spelling. Measure paired bindings that are `var` at rule entry, report
loss against the conversions the baseline actually made, and break results
down by build-target group. Existing `let` and `const` bindings must not dilute
the denominator. Keep fixture-specific evidence in the private investigation
records.
