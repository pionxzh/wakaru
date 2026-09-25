# Rewrite Assumptions

See also: [Architecture](architecture.md) for pipeline stages and rewrite
levels, [Rule dependency inventory](rule-dependency-inventory.md) for per-rule
safety classifications.

## Purpose

`RewriteLevel` controls how aggressively wakaru recovers original source, but
it does not explain *why* a particular rewrite is safe or unsafe. Two rules at
the same level may depend on completely different properties of the input.

This document names those properties. When a rule relies on something that is
not provable from the AST alone, it should say which assumption it depends on.
The goal is a shared vocabulary so rule authors make the same tradeoff the same
way, and so users can eventually understand what "standard" is actually
promising.

## Reproduce First

A new generated-code recovery should start from a reproduced compiler, bundler,
or minifier shape. Prefer a small input snippet plus the tool and version that
produced the lowered code.

Good sources: Babel, TypeScript, SWC, esbuild, terser, webpack, Rollup, and
emitted helper/runtime code from real packages.

A bug report is useful evidence, but it should not by itself justify a new
heuristic if the producing tool and shape cannot be reproduced. Patterns that
look generated but cannot be traced to a known toolchain belong in `aggressive`
at most, with a test comment noting the shape is speculative and why
reproduction was unavailable.

## Assumptions

These are named properties of the input that rules may depend on when a
transform is not provable from the AST alone.

Rules should reference these names in code comments or test names when
applicable, so the dependency is grep-able.

### `call_receiver_independence`

A callable recovered from generated helper or module syntax does not depend on
the incidental receiver that the lowered representation introduces or removes.

For example, these call shapes differ in ordinary JavaScript because their
`this` values differ:

```js
(0, namespace.fn)(); // `this` is undefined
namespace.fn();      // `this` is namespace

wrapped.default();   // `this` is wrapped
defaultImport();     // `this` is undefined
```

Transpilers deliberately emit some of these receiver forms while lowering ESM,
and interop wrappers introduce others as an implementation detail. Recovering
the pre-transpile import call may therefore change the behavior of a callable
that observes `this`, even though the recovered form matches the original ESM
source shape.

Affects: `UnIndirectCall` (member-callee forms), `UnInteropRequireDefault`
(call sites rewritten from `.default`), and `UnEsm` (default interop recovery
and conditional named-export reads rewritten from `exports.fn()` to `fn()`).

Level: receiver-changing `UnIndirectCall` and `UnEsm` forms require `standard`
or above. Explicit transpiler-helper recovery in `UnInteropRequireDefault`
applies whenever that helper is recognized.

### `iterator_materialization_independence`

The program does not depend on the eager iterator materialization introduced by
transpiler helpers, including its ordering relative to destructuring defaults.

For a recognized sliced-to-array helper and a complete default pattern,
`standard` prefers recovery of the pre-transpile source. Matching the helper's
literal limit `N` to the pattern's element count constrains the group shape; it
does not prove that iterator steps, defaults, and iterator closing happen in
the same order.

For example, start each variant with this state:

```js
let nextValue = 1;
function fallback() { nextValue = 99; return 7; }
function* values() {
  yield undefined;
  yield nextValue;
}
```

Babel's helper first materializes both values and closes the iterator, then the
lowered code evaluates the default:

```js
const tmp = _slicedToArray(values(), 2);
const a = tmp[0] === undefined ? fallback() : tmp[0];
const b = tmp[1]; // 1
```

The recovered pattern evaluates the default before requesting the second value:

```js
const [a = fallback(), b] = values(); // a = 7, b = 99
```

This difference was reproduced with `@babel/plugin-transform-destructuring`
7.28.5 using its default options, without `loose` or extra compiler assumptions.
It affects later iterator values as well as `IteratorClose` timing; equal `N`
is not an equivalence proof. Recovering the original source can therefore change
the behavior of the compiled input in this edge case.

This assumption does not relax helper identity, reassignment, temporary-use,
or complete-pattern checks. An unproven or reassigned helper call must remain.
See [helper detection](helper-detection.md) for those proof requirements.

Compressed indexed returns recovered into complete patterns share the helper
versus native iterator boundary. For example, TypeScript's `__read` calls the
iterator's `return()` if a later `next()` throws after an earlier successful
step; native destructuring propagates that error without closing the iterator.
Recovering the pattern at `standard` accepts this source-recovery difference.

Affects: `UnDestructuring` (complete sliced-helper groups with defaults) and
`UnSlicedToArray` (compressed indexed returns recovered into complete patterns).
`UnParameters2` may subsequently fold the recovered pattern into a parameter.

Level: `standard` and above. `minimal` retains the helper materialization for
these default groups and compressed indexed returns. These recoveries use the
existing source-recovery policy; helper identity and complete-pattern checks
remain required.

### `async_iterator_value_await`

The program does not depend on the extra `await` that Babel 7.8–7.13 apply to
each iteration value of a lowered `for await`.

Every lowerer replaces `for await (const item of iterable)` with an adapter call
(`_asyncIterator`, `_async_iterator`, `__forAwait`, `__asyncValues`) and a
`try`/`catch`/`finally` protocol that calls the iterator's `return()` on an
abrupt exit and rethrows the body's error after it. Native `for await` performs
the same `IteratorClose` and error propagation, so folding the protocol back is
not an assumption. Babel 7.8–7.13 additionally emit
`value = await step.value` in the loop head; native `for await` awaits only the
result object of `next()`, not its `value`. For an async iterator that yields
promises as values, the lowered loop observes the settled value while the
recovered loop observes the promise. Spec-conformant async iterators do not
yield promises, and the `AsyncFromSyncIterator` wrapper of later Babel
versions and of the runtime already awaits sync iterator values, so this is a
source-recovery difference only for that producer range.

Affects: `UnForOf` (`for await` recovery from the Babel 7.8–7.13 protocol).
The other protocols carry no extra `await` and are recovered without this
assumption.

Level: `standard` and above, with `UnForOf`.

### `no_document_all`

The input does not depend on the legacy `document.all` falsy-object behavior.

Loose nullish checks:

```js
x == null    // true for null, undefined, AND document.all
x != null
```

are not strictly equivalent to `x === null || x === undefined`. Optional
chaining and nullish coalescing recovery from loose checks depends on this
assumption.

Affects: `UnOptionalChaining` (loose null-check forms), `UnNullishCoalescing`
(loose null-check forms).

Level: `standard` and above. `minimal` should only recover optional chaining
and nullish coalescing from strict checks or temp-based patterns where the
assumption is not needed.

### `pure_getters`

Property reads on the rewritten base are stable and side-effect-free.

This matters whenever a rewrite changes how many times a property is read:

```js
// input: two reads of obj.value
obj.value != null ? obj.value : fallback

// output: one read of obj.value
obj.value ?? fallback
```

If `obj.value` is a getter with side effects, the rewrite changes observable
behavior.

The same applies to optional chaining recovery:

```js
// input: two reads of obj.a
obj.a != null ? obj.a.b : undefined

// output: one read of obj.a
obj.a?.b
```

Temp-based patterns avoid this entirely - the original code already evaluates
the property once:

```js
var _a;
(_a = obj.value) != null ? _a : fallback
// -> obj.value ?? fallback (safe: _a proves single evaluation)
```

Rules should prefer temp-based recovery when available. Repeated-access recovery
requires this assumption.

Affects: `UnOptionalChaining` (repeated-base forms), `UnNullishCoalescing`
(repeated-base forms).

Level: `standard` and above for identifier bases (e.g. `x.prop`). Member
expression bases (e.g. `a.b.prop`) should require `aggressive` unless a temp
proves single evaluation.

### `stable_builtins`

Global builtins and their methods are not patched between an alias capture and
its later use.

Minifiers often create aliases to save bytes:

```js
const O = Object;
const E = TypeError;
const def = Object.defineProperty;
```

Inlining those aliases changes when the global or property is read:

```js
const E = TypeError;
patchTypeError();
throw new E("x");        // uses captured TypeError
throw new TypeError("x"); // reads TypeError after patchTypeError()
```

That is usually acceptable for generated production bundles, but it is not a
semantic guarantee from the AST alone.

Affects: `UnBuiltinAliases` and `SmartInline` (builtin/global alias
inlining).

Level: `standard` and above. `minimal` preserves captured builtin aliases.

### `terser_unsafe_proto`

The literal receiver in a prototype-call shape came from Terser's
`unsafe_proto` compression and may be reversed to the corresponding builtin
prototype:

```js
Array.prototype.splice.apply(value, args)
// terser unsafe_proto ->
[].splice.apply(value, args)
```

Terser keeps `unsafe_proto` disabled by default and applies it only when the
builtin reference is undeclared. Wakaru does not retain that producer
provenance. Reversing the shape therefore assumes the synthesized `Array`,
`String`, `Object`, `Number`, `RegExp`, or `Function` identifier still resolves
to the intended builtin. The rule deliberately does not model lexical
bindings, `with`, or direct `eval` to prove that condition.

Affects: `UnBuiltinPrototype`.

Level: `aggressive` only. `minimal` and `standard` preserve the literal
receiver.

### `effect_free_property_key_coercion`

Converting a generated property-definition helper back to a computed object
property assumes coercing the property key has no observable side effects.

```js
_defineProperty({}, key, makeValue()); // arguments, then helper coerces key
({ [key]: makeValue() });              // coerces key before evaluating value
```

Babel and SWC emit the helper call while lowering `{ [key]: value }`, and
ordinary string, number, and symbol keys make the two orders equivalent. A
key object with a side-effecting `Symbol.toPrimitive`, `valueOf`, or `toString`
can observe the difference.

Affects: `UnDefineProperty` for expression-position calls whose target is an
exactly empty object literal. Standalone calls rewritten to assignments do not
depend on this assumption.

Level: `standard` and above. `minimal` preserves the helper call.

### `string_coercion_hint`

Recovering a template literal from a string-literal-led `+` chain assumes the
substituted values coerce to the same string under ToPrimitive hint `default`
(what `+` uses) and hint `string` (what a template uses).

```js
"Hello " + name + "!"   // ToPrimitive(name, "default"): valueOf first
`Hello ${name}!`        // ToString(name): toString first
```

Evaluation order is identical in both forms — each operand is evaluated and
coerced before the next is evaluated — so the hint is the only difference. It
is observable for objects whose `valueOf` returns a primitive that differs from
their `toString` (date/time libraries such as moment, dayjs, and luxon return a
timestamp from `valueOf`), for `Symbol.toPrimitive` implementations that branch
on the hint, and for objects whose `valueOf` throws (Temporal). Every built-in
coerces identically: `Date` treats the default hint as `string`, and primitive
wrappers, arrays, plain objects, and symbols behave the same either way.

Babel loose mode and TypeScript ≤ 4.4 lower templates to this exact shape, so
the reversal restores the original template where the chain was generated — but
the shape is indistinguishable from handwritten concatenation, so it is an
assumption, not a proof. The private fixture suite recovers roughly 3,000
templates through this path with no substitution shaped like a known
hint-sensitive object, which is why `standard` keeps it rather than demoting it
to `aggressive`.

Affects: `UnTemplateLiteral` (plus-chain path).

Level: `standard` and above. `minimal` rewrites only chains whose substitutions
are primitives by syntax (literals, nested templates, unary/update/arithmetic
and comparison results, and conditionals or logical operators over those),
where ToPrimitive is the identity and the hint cannot be observed.

### `concat_coercion_order`

Recovering a template literal from a string-literal-led `.concat` chain assumes
that coercing one substitution has no effect a later substitution's evaluation
can observe.

```js
"a".concat(first(), second())  // evaluates both calls, then coerces both
`a${first()}${second()}`       // coerces first() before evaluating second()
```

Under the execution-environment baseline (the `concat` being called is the
intrinsic `String.prototype.concat`), the method coerces with ToString, the
same hint a template uses, so the coercion result is identical; only the
interleaving of evaluation and coercion differs. It is observable only when a
substitution's `toString` / `Symbol.toPrimitive` has side effects that a later
substitution reads. A patched `concat` is outside the baseline — `minimal`'s
primitive-only rewrite is exact with respect to coercion, not with respect to a
replaced method.

The string-literal receiver is strong producer evidence — Babel (spec mode),
SWC, esbuild, and TypeScript ≥ 4.5 all lower templates this way and handwritten
code almost never calls `.concat` on a literal — but the AST cannot prove the
producer, so this remains a named assumption.

Affects: `UnTemplateLiteral` (concat-chain path). Tagged-template helper
recovery is a separate, provenance-checked path and does not depend on this.

Level: `standard` and above. `minimal` rewrites only chains whose substitutions
are primitives by syntax, as for `string_coercion_hint`.

### `concat_arguments_are_arrays`

Unknown arguments in an array-literal `.concat(...)` call are ordinary arrays,
so concat's conditional flattening can be recovered as array spread:

```js
[head].concat(items, [tail])
// ->
[head, ...items, tail]
```

This is not true for an arbitrary value. Concat appends a scalar or string as
one element, and spreads only arrays or values opting in through
`Symbol.isConcatSpreadable`; array spread instead requires an iterable and
always iterates it. Babel's loose / `iterableIsArray` transforms emit this
concat shape after assuming their spread inputs are arrays, but the resulting
AST no longer carries that producer setting.

Affects: `UnArrayConcatSpread` for arguments whose array identity is not proven.
Array literals are known directly. A separate binding proof covers rest
parameters, canonical Babel/TypeScript `arguments`-copy arrays, bindings
initialized with a hole-free array literal, and calls to functions whose whole
body returns one. Every other use of a proven value binding must be an
intrinsic-concat operand, and every use of a proven function a direct call, so
these forms do not depend on this assumption.

Level: `aggressive` only. `minimal` and `standard` preserve unknown concat
arguments; `standard` may still recover the proof-backed forms.

### `set_computed_properties`

Folding a sequence of member assignments back into an object literal assumes
that *assigning* each property is equivalent to *defining* it.

```js
var _n;
var n = (_n = {}, _n[k] = 1, _n.b = 2, _n); // assignment: hits inherited setters
var n = { [k]: 1, b: 2 };                   // definition: always own properties
```

This is the same assumption Babel exposes as
`@babel/plugin-transform-computed-properties` `loose: true` / the Babel 7
`setComputedProperties: true` assumption, which is what produces the shape in
the first place. Key evaluation order is *not* at risk — both forms evaluate
each key before its own value, in source order.

Several cases can still observe the difference:

- An inherited setter handles the assignment instead of creating an own
  property. An inherited getter-only or non-writable data property can likewise
  make the assignment fail or do nothing. These prototype descriptors are
  environment-driven and cannot generally be decided from the AST.
- The key is `__proto__`: `obj.__proto__ = x` invokes the inherited setter and
  changes the prototype, while `__proto__` in a computed key position defines
  an own property. Statically-known `__proto__` keys, including
  no-substitution template literals, are rejected outright; a dynamic key that
  evaluates to `"__proto__"` at runtime is covered by this assumption.
- Object-literal evaluation infers a `.name` for an anonymous function or class
  value from its property key, while the preceding member-assignment form does
  not. This is visible in the AST, but preserving every such assignment would
  defeat recovery for otherwise ordinary loose-transform output, so
  `standard` deliberately covers that difference.

The rule also rejects, at every level, a seed literal containing an accessor
or a `__proto__` key, and any temporary that is observable outside the matched
pattern (see "Generated Temporaries" below).

Affects: `UnComputedProperties`.

Level: `standard` and above. `minimal` preserves the sequence.

### `transpiled_class_accessor_attributes`

Recovering an accessor descriptor inside a proven class-lowering IIFE assumes
that its attributes describe the original source construct rather than an
intentional handwritten descriptor. TypeScript 3.5–3.8 lowered class accessors
with `enumerable: true, configurable: true`; TypeScript 3.9 changed that output
to the native class attributes (`enumerable: false, configurable: true`).

At `standard` and above, `UnEs6Class` accepts both variants after the enclosing
IIFE has independently matched a transpiler class shape. This preserves source
recovery for older TypeScript even though reflecting on the emitted descriptor
can observe the enumerability difference. `UnPrototypeClass` has no equivalent
producer proof and accepts only the native class attributes. A missing or false
`configurable` flag is rejected at every level.

Affects: `UnEs6Class` direct `Object.defineProperty` accessor recovery.

Level: `standard` and above. `minimal` requires attributes exactly representable
by class syntax.

### `native_class_inheritance`

The program does not depend on the inheritance-emulation details introduced
by downlevel compilation.

For a proven TypeScript default derived constructor, `standard` prioritizes
recovering the native class source over preserving every behavior of the ES5
inheritance emulation. TypeScript 5.9.3 emits this constructor even when the
source has no explicit constructor:

```js
function Child() {
  return base !== null && base.apply(this, arguments) || this;
}
```

Recovering `class Child extends Parent { ... }` removes this constructor and
uses native construction. It also recovers proven instance/static superclass
method calls as `super.method(...)`. These changes are intentionally observable:

- With `Parent = null`, the lowered constructor can return `this`; the native
  default derived constructor throws when instantiated.
- With a native class as `Parent`, `.apply` throws, whereas native construction
  can succeed. Built-in constructors can also behave differently.
- Overriding a parent's `.apply` or a method's `.call` affects the lowered code,
  but native construction and `super.method(...)` bypass those properties.
- Replacing `Parent.prototype` after the child is defined affects the lowered
  `base.prototype.method` lookup. Native `super` follows the child method's
  home object's prototype instead. Mutating that prototype chain can expose
  the opposite difference.

This is a source-recovery assumption, not evidence that the runtime parent is
an ordinary function or that the two programs are execution-equivalent. The
usual function-to-class changes (including requiring `new`, strict methods and
non-enumerable methods) also apply. Recognition remains bounded to the compiler
frame and stable helper bindings; this does not authorize deleting arbitrary
null guards or rewriting captured superclass references.

Affects: `UnEs6Class` recovery of the TypeScript default derived constructor
and its instance/static superclass method calls. See
[helper detection](helper-detection.md#typescript-default-inheritance) for
recognition and rejection boundaries.

Level: `standard` and above. `minimal` preserves this lowered constructor and
wrapper. Other existing class recoveries have their own boundaries; this is
not a claim that `minimal` preserves every lowered class.

### `commonjs_exports_data_properties`

Properties that compiler-emitted code writes on the module's own `exports` /
`module.exports` object are ordinary data properties. No accessor installed on
that object runs when such a write is reordered or repeated. The wrapper's
`module.exports` slot is also an ordinary data property, and the export
receiver is not a Proxy. Reading that slot therefore neither runs code nor
returns a different object unless a write replaces it.

CommonJS does not guarantee this. A module may install a setter on its own
export object:

```js
let value = 1;
Object.defineProperty(exports, "b", { set(v) { value = 2; } });
exports.a = exports.b = value; // chain: reads value once, a receives 1
exports.b = value; exports.a = value; // split: a receives 2
```

In the transpiler output examined so far and in the private fixtures, an
accessor on `exports` never appears together with a chained export assignment:
TypeScript's `exports.A = exports.B = void 0` and Babel's
`exports.default = exports.x = value` are emitted against a fresh object whose
accessors, if any, are live re-export getters on other keys. That is an
observation, not a guarantee. The hazard is accepted rather than proven; the
CommonJS wrapper only guarantees that `module`, `exports`, and `require` are
defined.

Affects: `UnAssignmentMerging` (repeated identifier values and stable
CommonJS receivers, including chains made entirely of static
`module.exports.name` stores), and `UnEsm` (every `exports.x = v` to `export`
recovery replaces a property write with a binding, so an accessor on `exports`
is already ignored; its whole-chain recovery also evaluates a chained
function value once into a binding and moves each target's reference
evaluation past it, which creating a function cannot observe; its conditional
named-export recovery replaces each eligible property with a live binding at
the original read/write positions). A primitive literal or `void <number>`
needs no assumption about the repeated value, but receiver stability is a
separate condition. Nested receiver chains that also replace
`module.exports`, mutate prototypes, or write the `exports` key stay intact;
splitting them would require a stronger proof or a receiver capture. A
conditional named-export chain may mix static `exports.name` and
`module.exports.name` roots only after proving that neither receiver can be
rebound, replaced, aliased, or observed dynamically.

Level: all levels. UnEsm's recovery is itself unconditional on this point.

### `chain_receiver_reference_order`

Recovering a chained named-export assignment as one operation evaluates the
value first and the target references afterwards. The original chain
evaluates every target reference (`exports`, `module.exports`) before the
value. The two orders differ only if evaluating the value synchronously
rebinds `exports` or replaces `module.exports`; the property writes
themselves happen in the same order, to the same objects, with the value
evaluated exactly once in both forms.

```js
var Lib = require("lib");
exports.first = exports.second = Lib.matcher(KEY);
// recovered: the value is stored first, then each target reads the receiver
var second = Lib.matcher(KEY);
exports.second = second;
exports.first = second;
```

Rebinding `exports` needs a write to that parameter, which only code lexically
inside this module can perform. Replacing `module.exports` needs the `module`
object, which only this module's code holds unless it passes `module` out. A
call on a provider binding (a top-level `require("literal")` declarator, or a
static member chain rooted at one, declared once and never written) with
identifier or literal arguments therefore cannot rebind either receiver on its
own. It can do so only by calling back into a closure this module registered
earlier, or through an escaped `module`, and the single-export recovery
(`exports.name = Lib.make(x)` to `export const name = Lib.make(x)`) already
accepts those channels without inspecting the module for them. The chain
recovery accepts them on the same terms and does not widen them: arguments
that are the wrapper bindings `module`, `exports`, or `require`, computed
callee keys, spread arguments, optional calls, `new`, and every call whose
callee root is a local function, object, or an unresolved global stay whole.
The value stays at the chain's own position; unlike the `require("literal")`
value (`import_hoisting_eagerness`), nothing is hoisted.

This is an accepted assumption in the same sense as
`commonjs_exports_data_properties`: it names the residual rather than proving
it absent. A local call (`makeValue()`) is excluded not because the argument
fails but because a local function body that writes `module.exports` is a
realistic shape, while a provider re-entering this module's receivers is not.

Affects: `UnEsm` whole-chain recovery of top-level named-export chains whose
value is a provider call. Everything the recovery does afterwards (binding
name, snapshot exports, `module.exports` head) is the existing function-value
path.

Level: all levels, matching the rest of the chain recovery.

### `import_hoisting_eagerness`

Converting a CommonJS `require()` into an ESM `import` moves the provider's
evaluation ahead of every consumer statement: imports are hoisted and all
dependencies evaluate before the importing module's body. In CommonJS, a
provider executes at its `require` call site, interleaved with the consumer's
own statements. Every wakaru CommonJS recovery shares this deviation; it is
observable whenever a later provider's side effects (a global write, an
installed getter or setter) change what an earlier consumer statement — such
as an `Object.assign` copy — reads. Relative provider order is preserved;
only the provider-versus-consumer interleaving moves.

Recoveries that copy values at a specific program point (the default-object
composition's `Object.assign` shells) prove the consumer's body exact but
prove providers only at their export surface: a provider may run arbitrary
side-effect statements before its single default assignment. Proving
providers side-effect-free would reject essentially every real module for a
hazard every `require`-to-`import` conversion in this codebase already
accepts.

Affects: `UnEsm` require conversion, `commonjs_default_object_composition`,
and every fact-consuming recovery that imports a proven provider. UnEsm's
whole-chain recovery of `exports.a = exports.b = require("x")` evaluates the
provider once into a binding before the export writes and then converts it
the same way; it accepts this ordering deviation and does not prove that the
provider leaves the consumer's `module.exports` slot untouched. The esbuild
unpacker's ownership relocation (moving a top-level state writer into the
module that owns the state declaration) shifts that statement's evaluation to
the owner's import time — the same provider-versus-consumer interleaving
deviation.

Level: all levels. This is inherent to emitting ESM from CommonJS.

## Execution Environment Baseline

Every level assumes the program runs in a standard ECMAScript environment:
intrinsic objects, global bindings, and prototype methods retain their
specified behavior. wakaru does not model mutations performed by opaque calls,
other scripts or modules, host code, other realms, or dynamically evaluated
code, and there is currently **no pipeline-wide mutation detection** — an
explicit `String.prototype.concat = ...` in the same input does not stop the
`.concat` rewrite. Individual rules may fail closed on a mutation directly
visible to their own matcher; that coverage is rule-specific hardening, not a
contract. `minimal` reduces speculative source recovery; it is not a guarantee
against a modified runtime.

This is what lets a rule treat an unresolved `Math`, `Object`, `Array`,
`Promise`, or `String.prototype.concat` as the intrinsic: `Math.pow(a, b)` →
`a ** b`, `"a".concat(b)` → `` `a${b}` ``, `Object.assign({}, x)` → spread,
`new (P || (P = Promise))` → a native `async` function. Resolver identity is
still required — a local binding named `Math` is never the intrinsic. A future
`EnvironmentHazards` pre-pass over the resolved original AST could record
directly visible same-input mutations for every rule; once it exists, this
section should narrow the baseline to exclude them. It is deliberately
distinct from `stable_builtins`, which is narrower: that
assumption says a builtin is not patched *between an alias capture and its
later use*, because alias inlining moves the lookup in time. The baseline says
the intrinsics are intact to begin with.

Rationale: requiring each rule to prove the whole realm untouched would reject
essentially every real toolchain shape for a hazard wakaru cannot observe
anyway. The trade is recorded here once instead of being re-argued per rule.

## Generated Temporaries

Temporaries introduced by compilers are handled by binding analysis, not by
assumption. A temporary may be removed only when reference analysis proves it
is isolated to the matched pattern:

```js
var _tmp;
const out = (_tmp = obj.value) == null ? fallback : _tmp;
// -> const out = obj.value ?? fallback
// safe: _tmp has no reads or writes outside the pattern
```

If the temp is observed elsewhere, no level or assumption overrides that:

```js
var _tmp;
const out = (_tmp = obj.value) == null ? fallback : _tmp;
console.log(_tmp);
// _tmp escapes the pattern - do not remove
```

This is a hard rule, not a level-gated policy. It prevents the assumption
system from becoming a mechanism to skip safety checks.

`TempIsolation` in `rules/binding_facts.rs` implements this proof. A temp is
isolated when the module has no other use of it and its only declaration is
an uninitialized declarator a pattern may assign. That excludes parameters,
which sloppy-mode `arguments` aliases, and a `let` written in its TDZ. It
also excludes an exported declaration, which importers can read, and an
ambient `declare var`, which creates no binding.

Every rule that can drop a temp's write passes each rewrite through
`TempIsolation::accept_expr_rewrite` (or `accept_stmts_rewrite`) at its
choke point. The rewrite is rejected if it drops a write to a binding that
is not isolated to the rewritten input, or if its output still reads that
binding. This covers code paths that have no pattern proof of their own. An
accepted rewrite reports which declarations became dead, for
`remove_consumed_uninitialized_decls`, and updates the use counts for later
rewrites. A pattern's own count check, where it has one, is an early exit and
a shape policy. It does not replace this check.

`SmartInline` applies a separate, position-independent proof to generic
single-read `const` aliases. It only removes generated-looking names used in
the immediately following statement whose identifier source is definitely
initialized in the current function/statement-list
context: a parameter or catch binding, a local function declaration, or a
same-list declaration above the capture. The source must have no same-scope
writes after capture and no writes in any deferred body, including parameter
defaults and object accessors. Imports (live bindings), unresolved globals, and
outer lexical bindings are excluded; direct `eval` or `with` also blocks the
rewrite. Parameters are also excluded when their containing function observes
`arguments`, because sloppy-mode mapped arguments can write a parameter without
an identifier assignment. A replacement is rejected when a different binding
with the same emitted name occurs in the use statement, preventing the printed
identifier from being captured after `SyntaxContext` is erased. The unresolved
global `undefined` is the only global exception.
An entry-binding proof may flow into nested lexical blocks in the same
activation, but never into a constructor, static block, or object accessor
statement list analyzed under a different activation/order domain.

Existing `let` aliases stay even when never written: `SmartRename` runs later
and may recover a meaningful name from their use sites, which SmartInline
cannot predict cheaply. The generated-name check for `const` is readability
policy as well as a safety gate.
Wakaru removes `const o = source` when proven safe, but preserves names such as
`const snapshot = source` or `const store = importedBinding` because those names
carry useful recovered intent. It also preserves long-lived short aliases
because SmartRename may recover intent from their later use. This rule
deliberately does not simulate
expression evaluation order: once the local source is proven frozen, delaying
its read is harmless; otherwise the alias stays.

Candidate declarations still participate in reference counting. Replacement
chains are resolved to their surviving source before substitution, and that
source's emitted name is checked again at each alias's use. If it would be
captured there, the alias stays declared while its initializer receives safe
substitutions for earlier links.

## Declaration-Kind Capture Safety

`VarDeclToLetConst` treats references to local function declarations as possible
execution or escape, regardless of invocation syntax (`f()`, `f.call(...)`,
`f.apply(...)`, callback arguments, or alias creation). Resolver binding IDs
connect references between declarations in the same function/module scope.
Captures include nested closures, parameter defaults, computed keys and class
members. Anonymous closures and class expressions are conservatively observed
at creation; the rule does not follow aliases through properties or model
individual APIs.

Named class declarations can defer their captures until a local value
reference only when creation cannot execute those captures: no superclass,
computed keys, decorators, static fields or static blocks. Constructors,
ordinary/private methods (including static methods), and non-computed instance
fields are deferred. Other class shapes remain exposed at creation. In
particular, a static block or initializer can invoke `this.method()` without
referencing the class identifier; it must not use the deferred path.
An earlier reference cannot execute a simple class's captures before that
class initializes: its own TDZ prevents access. The ordered scan queues such
references until the class declaration completes, then expands its summary.
This also applies to transitive references from hoisted functions or classes.

A captured `var` can become lexical only when its declaration has completed
before the exposure in a containing statement-list block. This includes
ordinary nested blocks, but does not infer initialization across branches,
`switch` cases, or loop headers. Existing block-escape, loop-capture, duplicate
declaration and dynamic-scope guards still apply. A plain function/arrow
initializer may reference its own binding: creating it cannot execute its
body before initialization. Calls and class initializers do not get that
exception.

The reference graph expands each declaration summary once at its earliest local
exposure; it does not build a transitive capture set for every function.
Analysis is bounded to each scope and reuses one capture traversal for named
functions/classes and anonymous function-like values. Enclosing scopes still inspect
nested bodies for captures, so this is not a claim of globally linear AST
processing across arbitrarily deep function nesting.

Pure ESM export specifiers (`export { f }`, including aliases/default names)
link bindings without evaluating their values, so they do not add a local
exposure. A default-export expression consisting only of a parenthesized or
bare identifier bound to a local function declaration or simple class
declaration also adds no capture exposure. Unlike a specifier's live binding,
`export default f` evaluates and snapshots the binding value, but does not
execute its body. The value read and any class TDZ error remain in the output;
ordinary variables still undergo use-before-declaration checks. This exception
does not include calls, object/array literals, sequences, aliases or anonymous
functions/classes. Property stores and getter closures still expose captures.
Named function declarations retain the existing declaration-position capture
guard, independently of whether they are exported. Simple named class
declarations use the deferred boundary above, including named/default exports;
anonymous default classes remain exposed at creation.

This proof does not add cross-module entry roots for every exported function
or class. It retains `minimal`'s exported-`var` preservation. Arbitrary ESM-cycle
entry before module execution requires a separate cross-module policy;
same-scope analysis is not such a proof.

Accepted residual at every level: storing a function in an object property
before its captured variable is initialized can preserve `var` even when the
function actually runs only later. This includes compiler-emitted lazy
CommonJS wrappers whose exports object is a local alias. An exports-like name
is not proof: a same-scope member call or setter can invoke that function early.
We deliberately do not track object aliases or assume delayed property use,
including at `aggressive`. This can also prevent downstream destructuring or
name recovery. It is a known readability cost of the bounded proof, not a
claim that each preserved `var` fixes an observed runtime failure. See the
[cached-wrapper capture boundary](learnings/cached-wrapper-capture-boundary.md)
for the alias, reentry and exceptional-exit counterexamples behind this decision.

## Dynamic Scope Limits

wakaru does not fully model `eval`, `with`, or host-level observation of
generated temporaries (e.g. top-level script `var` bindings leaking to
`globalThis`).

Rules should still perform binding/reference analysis within the containing
function or module scope. They do not need to bail out of otherwise valid
recovery because dynamic code could theoretically observe an isolated compiler
temp.

Original bindings are different from compiler temps: a rule that renames,
removes, or re-kinds a binding the input program declared (params, vars) can
break code a direct `eval` evaluates. Binding-oriented rules guard
conservatively via `rules/eval_utils.rs`: `DirectEvalAnalyzer` classifies
direct/indirect eval calls and their sources, and
`js_source_mentions_binding` scopes the bail-out to bindings a known source
string mentions (an unknown source blocks all). `VarDeclToLetConst`,
`DeadDecls`, and `UnIife` follow this pattern. `ArrowFunction` preserves the
function shape for unknown direct-eval sources, or when a known source mentions
function-only bindings such as `this`, `arguments`, and `new.target`. Nested
regular functions have their own function-only bindings and do not block an
outer conversion; nested arrows still do. For `function() {}.bind(this)`, a
source that mentions only `this` is safe because both forms capture the same
value, while `arguments` and `new.target` still block conversion.

`with` and direct `eval` are module-wide hazards. A rule that reads a free
name as the global, renames or removes a binding, or introduces a new binding
skips the whole module when either construct is present. wakaru does not model
`with` bodies or eval scopes any finer than that: compilers do not emit them,
and well under 1% of modules in a huge corpus of production bundles contain
either. A rule that lacks the check is a bug, not a documented exception.

Two boundaries follow from this.

- `SyntaxContext` covers static lexical scope only. It does not see bindings a
  `with` object or a sloppy direct `eval` introduces at runtime. Unknown direct
  `eval` blocks renames, binding removal, declaration-kind changes, and
  synthesized identifier insertion; a known source string keeps the
  name-mention best effort above.
- Indirect `eval`, opaque calls, other modules, and host code can mutate
  globals and intrinsics but not the current lexical scope. Those mutations
  fall under the Execution Environment Baseline and are not tracked.

This limitation should be documented for users, especially for `minimal`.

## Rule Author Checklist

Before adding or widening a rewrite:

1. Reproduce the lowered shape from a known toolchain, or place the rewrite in
   `aggressive` and note the shape is speculative.
2. Decide the lowest level where the rewrite belongs.
3. If the transform is not provable from the AST alone, name the assumption it
   depends on (`no_document_all`, `pure_getters`) in the test or a code comment.
4. Prefer binding/reference proof over assumptions. A temp that proves single
   evaluation is better than relying on `pure_getters`.
5. Never let an assumption override a concrete observed use - a temp read
   outside the matched pattern means the temp stays.
