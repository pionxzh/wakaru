# Rewrite Assumptions

See also: [Architecture](architecture.md) for pipeline stages and rewrite
levels, [Rule dependency inventory](rule-dependency-inventory.md) for per-rule
safety classifications.

This document states contracts in terms of behavior. It names a rule or pass
by what it does, not by file or function; code pointers belong in
[Architecture](architecture.md) or [Code map](code-map.md).

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

## Source-Intent Recovery

Compiled CommonJS sometimes behaves differently from the ESM source it was
compiled from: an interop helper sets `default` to the whole provider, a
lowered `import()` loads synchronously, a call gains a receiver. A rule may
then emit the source's form, with ESM semantics, instead of reproducing the
CommonJS behavior. It may do so only when all of these hold:

- **Producer evidence.** The shape is reproduced output of a known compiler
  or bundler for a known source construct, recognized by what makes it that
  output: a helper body, a callback shape, an `__esModule` marker. Code that
  only resembles it keeps its CommonJS behavior.
- **The output matches the source.** The emitted form is what the producer
  started from, not a guess at what the author meant.
- **The difference is named.** An assumption below lists each behavior that
  moves from the CommonJS output to ESM semantics, and where the result
  differs for a real CommonJS provider.

Without that evidence, the rule keeps the CommonJS behavior, even when the
ESM form would read better. `namespace_interop_source_semantics`,
`lowered_dynamic_import_source_semantics`,
`webpack_lazy_import_source_semantics`,
`esbuild_lazy_import_source_semantics`, and
`relative_require_esm_provider` are recoveries of this kind, and so is the
`export *` recovery in [helper-detection.md](helper-detection.md).

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
and unpack's import of a module wrapped by a bundled interop helper (call
sites rewritten from `.default`), and `UnEsm` (default interop recovery,
and calls of property-storage exports rewritten from `exports.fn()` to `fn()`
unless a value written to that export is a function that reads `this`; calls
of mirror-storage exports rewritten to the local unless its declaration is a
function that reads `this`). Unpack's namespace decomposition rewrites a
consumer's `ns.fn()` to `fn()` on the same terms: it keeps the member call
when the provider's facts show that `fn` is a function that reads `this`. A
function that only forwards its receiver (`g.apply(this, arguments)`) counts
by what it forwards to: a same-module function, or the function literals
passed to a wrapping call, which is taken to pass the receiver on without
reading it, as Babel's async wrapper around `_asyncToGenerator` does. An
export whose value is such a wrapping call (Babel's lowered async function
expression, `_asyncToGenerator(function* () { … })`) counts the same way; a
call that takes no function literal does not count. A forwarding target the
module cannot see counts as reading `this`. A
recognized transpiler helper never counts: `_extends` and `__assign` forward
`this` to code that ignores it.

`UnIndirectCall` keeps `(0, o.m)()` and `Object(o.m)()` when `o` is a
same-module binding whose member `m` is an ordinary function that reads
`this` or `super` (an object literal method or function value, an `o.m =
function` assignment, or a static method of a class declaration or a class
value such as `var C = class {}`). Terser emits that shape for
`const m = o.m; m()` (producer `terser@5.51.2`), and the direct call would
pass `o` as the receiver. Import bindings, parameters, and other roots
whose callee the module cannot see still drop the wrapper.

Level: receiver-changing `UnIndirectCall` forms require `standard` or above,
in unpack mode too. `UnEsm` forms apply wherever `UnEsm` runs: `standard` and
above for single-file input (`single_file_esm_consumers`), and every level in
unpack mode. Below `standard`,
`(0, o.m)()` keeps its wrapper until `UnEsm` or unpack's namespace
decomposition replaces `o.m` with a binding, which has no receiver to lose. Explicit transpiler-helper recovery in
`UnInteropRequireDefault` applies whenever that helper is recognized around a
helper runtime `require`; around any other `require` outside a function or
class body it runs inside `UnEsm`, at the same levels. Inside one, the
`require` stays and so does the call.

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

TypeScript's `__read` also differs from native destructuring when closing the
iterator. If a later `next()` throws after an earlier successful step, `__read`
calls the iterator's `return()` before rethrowing; native destructuring
propagates the error without closing the iterator. Babel's `_slicedToArray`
matches native here (checked with `@babel/plugin-transform-destructuring`
7.28.5). This difference applies to every recovered `__read` group, not only to
groups with defaults: `UnSlicedToArray` recovers a plain
`var _a = __read(src, 2), a = _a[0], b = _a[1]` to `const [a, b] = src` at
every level, `minimal` included. Compressed indexed returns recovered into
complete patterns share the same boundary.

Affects: `UnDestructuring` (complete sliced-helper groups with defaults) and
`UnSlicedToArray` (every recovered `__read` group, plus compressed indexed
returns recovered into complete patterns). `UnParameters2` may subsequently
fold the recovered pattern into a parameter.

Level: the default-ordering difference and compressed indexed returns need
`standard` or above; `minimal` retains the helper materialization for those.
The `__read` close-on-throw difference is accepted at every level, because
`minimal` already recovers plain `__read` groups. These recoveries use the
existing source-recovery policy; helper identity and complete-pattern checks
remain required.

### `async_iterator_value_await`

The program does not depend on the extra `await` that Babel 6 and Babel 7
before 7.14.9 apply to each iteration value of a lowered `for await`.

Every lowerer replaces `for await (const item of iterable)` with an adapter
call (`_asyncIterator`, `_async_iterator`, `__forAwait`, `__asyncValues`) and a
`try`/`catch`/`finally` protocol that calls the iterator's `return()` on an
abrupt exit and rethrows the body's error after it. Native `for await` performs
the same `IteratorClose` and error propagation, so folding the protocol back is
not an assumption. Babel 6 and Babel 7 up to 7.14.7 additionally emit
`value = await step.value` in the loop head; 7.14.9 replaced it with the
abrupt-completion protocol. Native `for await` awaits only
the result object of `next()`, not its `value`. For an async iterator that
yields promises as values, the lowered loop observes the settled value while
the recovered loop observes the promise. Spec-conformant async iterators do not
yield promises, and the `AsyncFromSyncIterator` wrapper of later Babel versions
and of the runtime already awaits sync iterator values, so this is a
source-recovery difference only for that producer range.

Affects: `UnForOf` (`for await` recovery from the Babel ≤ 7.14.7 protocol).
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

Level: `standard` and above. `minimal` recovers optional chaining and
nullish coalescing only from strict checks, including Babel's temp-based
strict chains (`(_a = o.p) === null || _a === void 0 ? void 0 : _a.q`),
flattened `||` chains, and `&&` chains in a boolean context. A strict check
is exactly the `?.` test, so these need no assumption beyond the isolated
temps (see `pure_getters` and `intrinsic_function_call` for what else
`minimal` declines).

### `intrinsic_function_call`

The `.call` that a lowered optional call reads on the checked value is
`Function.prototype.call`.

Babel, TypeScript, and SWC lower `o.f?.(1)` to a temp and an explicit
receiver (TypeScript 5.9 output shown):

```js
(_a = o.f) === null || _a === void 0 ? void 0 : _a.call(o, 1)
// -> o.f?.(1)
```

The recovered call no longer reads `call`. For a function this is the same
call. For a callable object with its own `call` method, or a function whose
`call` was replaced, the lowered code runs that method and the recovered
code calls the value directly (a `TypeError` for a plain object).

Affects: `UnOptionalChaining` (every `.call(context, ...)` unwrap: ternary,
flattened, statement, and `if` forms).

Level: `standard` and above. `minimal` keeps the lowered form.

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

The checked value is compared structurally, so a computed key with side
effects (`a[f()]`, `a[i++]`) never matches its second read. The assumption
covers getters and proxies on the base only.

Affects: `UnOptionalChaining` (repeated-base forms), `UnNullishCoalescing`
(repeated-base forms).

Level:

- `minimal` recovers a repeated-read form only when the checked value is an
  identifier.
- `standard` also recovers a single ternary whose checked value is a member
  expression: `x.p`, `this.p`, `x.p.q`, `x[k]`, `arguments[i]`. This covers
  plain strict and loose `??` (`x.p != null ? x.p : d`), optional access
  (`x.p == null ? void 0 : x.p.q`), the member optional call
  (`x.p == null ? void 0 : x.p(1)` and its statement form
  `x.p == null || x.p(1)`), and the same shapes nested inside a lowered chain.
  Babel emits the member optional call in `loose` mode and under the
  `pureGetters` assumption (producer
  `@babel/plugin-transform-optional-chaining@7.28.5`).
- `aggressive` adds the multi-read shapes: flattened `||` chains whose root or
  final link reads a member again
  (`(_o = o) == null || _o.m == null ? void 0 : _o.m(a)`), `&&` chains whose
  first segment is a member, the `x.p === null || x.p === undefined || x.p`
  form of `UnNullishCoalescing`, and `??=` on a member deeper than
  `local.prop`.

An impure getter or a proxy on a base the module cannot see is the remaining
risk at `standard`. `arguments[i]` is always safe: its elements are data
properties.

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

The assumption covers patches the module cannot see, such as the call above.
When the module itself writes the alias's source path, or a prefix of it, the
alias stays at every level:

```js
var origLog = console.log;
console.log = function () { origLog.apply(console, arguments); };
// inlining `origLog` would make the wrapper call itself
```

Affects: `UnBuiltinAliases` and `SmartInline` (builtin/global alias
inlining).

Level: `standard` and above. `minimal` preserves captured builtin aliases.

### `builtin_callbacks_not_constructed`

A function passed inline as the callback of a built-in API is only called by
that API, never constructed. `ArrowFunction` turns a function into an arrow
only where its value provably never reaches `new` (an immediately invoked
callee, a binding that is only called, a callback whose same-module callee
only calls the parameter, an async function). Under this assumption it also
converts callbacks in these positions:

- the first argument of an unresolved `setTimeout`, `setInterval`,
  `setImmediate`, `queueMicrotask`, `requestAnimationFrame`, or
  `requestIdleCallback`, and the executor of an unresolved `new Promise`;
- the callback argument of a method named `map`, `forEach`, `filter`, `some`,
  `every`, `find`, `findIndex`, `findLast`, `findLastIndex`, `flatMap`,
  `sort`, `reduce`, `reduceRight`, `catch`, or `finally` (first argument),
  `then` (first two), or `replace` / `replaceAll` (second).

```js
items.map(function (item) { return item.id; });
// → items.map((item) => item.id)
```

The AST does not prove that the receiver of `.map` is an array or that the
global timer is the host's. A receiver whose own `map` or `then` constructs
its callback turns the conversion into a `TypeError`. The list is closed: a
name joins it with evidence that lowered code passes it callbacks, never as a
guess about a library API (`.on`, `.each`, and similar stay functions).

Affects: `ArrowFunction`.

Level: `standard` and above. The rule's other evidence is proven inside the
module and applies at every level.

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
provenance. Reversing the shape therefore assumes the literal receiver came
from this compression, not from handwritten code. The synthesized `Array`,
`String`, `Object`, `Number`, `RegExp`, or `Function` identifier must still
resolve to the builtin: the rule skips a name that any binding in the module
declares, and skips every name when the module contains `with` or a direct
`eval`.

The method read itself does not depend on the producer. When the
synthesized name resolves to the builtin, `[].m` and `Array.prototype.m` read
the same inherited property, so a same-module patch of `Array.prototype.m`
gives the same result in both forms. Two reads differ: `__proto__` is an
accessor (`[].__proto__` is `Array.prototype`, `Array.prototype.__proto__` is
`Object.prototype`), and an own property of the literal (`length`,
`lastIndex`) shadows the prototype's. The rule skips `__proto__` and dynamic
keys, which could name it at runtime; own properties hold non-callable
values, so `.call` throws in both forms. Babel minifies its helper templates with
`unsafe_proto` (producer `@babel/helpers@7.28.6`,
`lib/helpers-generated.js`), so inlined Babel helpers carry the shape even
in handwritten bundles.

Only the call form is recovered (`[].m.call(...)`, `[].m.apply(...)`). The
value form (`var push = [].push`) is not: there an own property such as
`[].length` gives a value instead of throwing, so a value-form recovery would
need to exclude own property names as well as accessors.

Affects: `UnBuiltinPrototype`.

Level: `standard` and above. `minimal` preserves the literal receiver.

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
exactly empty object literal, and `UnComputedProperties`, whose
member-assignment sequences also coerce each key after its value (see
`set_computed_properties`). Standalone calls rewritten to assignments do not
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

Several producers lower templates to this exact shape: Babel 7 and 8 in loose
mode, Babel 6 by default, SWC with `jsc.loose` from 1.2.155, TypeScript ≤ 4.4,
esbuild ≤ 0.12.5, and Closure Compiler targeting ES5. The reversal therefore
restores the original template where the chain was generated — but the shape is
indistinguishable from handwritten concatenation, so it is an assumption, not a
proof. The private fixture suite recovers roughly 3,000 templates through this
path with no substitution shaped like a known hint-sensitive object, which is
why `standard` keeps it rather than demoting it to `aggressive`.

Known miscompile: `"x" + dateObj`, where `dateObj` is a moment, dayjs, or
luxon object, becomes `` `x${dateObj}` ``. The input prints the timestamp from
`valueOf`; the output prints the formatted date from `toString`. No guard
covers it. Refusing call substitutions would catch `"x" + moment(t)` but not
`var m = moment(t); "x" + m`, and identifier substitutions are the common
case, so that guard costs recovery without closing the gap.

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

The string-literal receiver is strong producer evidence — Babel 7 and 8 in spec
mode, SWC without `jsc.loose`, esbuild ≥ 0.12.6, and TypeScript ≥ 4.5 all lower
templates this way and handwritten code almost never calls `.concat` on a
literal — but the AST cannot prove the producer, so this remains a named
assumption.

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
always iterates it. Several producers emit this concat shape after assuming
their spread inputs are arrays: Babel 6 and 7 in loose mode, Babel 7 with the
`iterableIsArray` assumption, TypeScript 1.5–3.5 targeting ES5 without
`downlevelIteration`, SWC with `jsc.loose` from 1.3.69, and Buble. The
resulting AST no longer carries that producer setting. Closure Compiler emits
the same receiver but wraps each spread argument in
`$jscomp.arrayFromIterable`, so its arguments are not unknown in this sense;
`UnArrayConcatSpreadRest` recovers them at `standard`, including the renamed
helper of ADVANCED output, recognized by its body.

The assumption does not cover `[].concat(x)` with a single argument. That
call is also the castArray idiom (`toArray(v) { return [].concat(v) }`),
which handwritten libraries use to wrap a scalar, and nothing in the call
tells the two apart. It stays a call unless the binding proof or a
Closure helper proves `x` is an Array. A loose `[...xs]` therefore stays
`[].concat(xs)` at every level.

An argument that is visibly not an array (a primitive literal or operator
result, a template literal, a function, or an object literal without computed
or `__proto__` keys) stays one element, as concat appends it. An object literal
that may be concat-spreadable, and the array-like `arguments` object, keep the
concat call.

Affects: `UnArrayConcatSpread` for arguments whose array identity is not proven.
Array literals and the arguments its binding proof covers (rest parameters,
`arguments` copies, and bindings or calls that provably yield a fresh array;
see `rules/un_array_concat_spread.rs`) do not depend on this assumption.

Level: `aggressive` only, and never for a single argument of an empty
receiver. `minimal` and `standard` preserve unknown concat arguments;
`standard` may still recover the proof-backed forms.

### `set_computed_properties`

Folding a sequence of member assignments back into an object literal assumes
that *assigning* each property is equivalent to *defining* it.

```js
var _n;
var n = (_n = {}, _n[k] = 1, _n.b = 2, _n); // assignment: hits inherited setters
var n = { [k]: 1, b: 2 };                   // definition: always own properties
```

This is the same assumption Babel exposes as
`@babel/plugin-transform-computed-properties` `loose: true` (Babel 6 and 7) /
the Babel 7 `setComputedProperties: true` assumption, which is what produces
the shape in the first place. Other producers emit it without any option:
TypeScript 1.5–5.9 targeting ES5 always does, and SWC does with `jsc.loose`
from 1.2.155. esbuild's minifier also folds handwritten assignment code
(`var c; c = {}; c[k] = v; return c`) into `return c={},c[k]=v,c`, so the shape
does not prove that the source used definition semantics. Key *expression*
order is not at risk: both forms evaluate each key expression before its own
value, in source order. Key *coercion* order differs. The assignment form runs
ToPropertyKey on the key after evaluating the value, while the literal form
runs it before the value in Node 24. JavaScriptCore (Bun 1.3) runs it after the
value in both forms, so engines already disagree on the literal. Only a key
object whose `Symbol.toPrimitive`, `valueOf`, or `toString` has side effects
can observe this, the same hazard `effect_free_property_key_coercion` names.

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
intentional handwritten descriptor. TypeScript before 3.9 lowered class
accessors, instance and static, with `enumerable: true, configurable: true`.
TypeScript 3.9.2, the first stable 3.9 release, changed that output to the
native class attributes (`enumerable: false, configurable: true`).

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

A subclass with field initializers and no constructor gets the same frame
stored in an alias, `var _this = <frame>; _this.x = ...; return _this;`.

Recovering `class Child extends Parent { ... }` removes the default
constructor, or rebuilds the field-initializer one as
`constructor(...args) { super(...args); this.x = ...; }`, and uses native
construction. It also recovers proven instance/static superclass
method calls as `super.method(...)`, in these classes and in classes whose
explicit constructor already became `super(...)` (below). These changes are
intentionally observable:

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
(with or without field initializers), and of instance/static superclass
method calls in those classes and in classes with an explicit constructor. See
[helper detection](helper-detection.md#typescript-default-inheritance) for
recognition and rejection boundaries.

In multi-module unpack, a base constructor that another module calls with
`.call` / `.apply` becomes a class at `standard` only when that caller is
predicted to become `class extends`; a wrong prediction is reported as
`cross_module_class_call` ([fact system](fact-system.md#rules-that-read-facts)).

Level: `standard` and above. `minimal` preserves this lowered constructor and
wrapper, keeps a wrapper whose methods make superclass method calls, and keeps
every cross-module `.call` / `.apply` target a function.
Other existing class recoveries have their own boundaries; this is not a claim
that `minimal` preserves every lowered class. In particular, an explicit
TypeScript constructor (`var _this = _super.call(this, m) || this`) in a class
without superclass method calls becomes `super(m)` at every level, `minimal`
included, and so does `_super.apply(this, args)` over a proven rest-argument
copy. That recovery carries the same native-parent and overridden-`.call`
differences listed above. With a native parent such as `Error` it also changes
`instanceof` and method lookup on instances, in the direction of the source.

### `class_members_not_constructed`

A function member that class recovery turns into a method is not constructed
where the module cannot see it. A method has no `[[Construct]]` and no
`prototype`, so `new Foo.Inner()` or `Foo.Inner.prototype.x = …` throws once
`Foo.Inner = function () {}` becomes `static Inner() {}`.

`UnPrototypeClass` and `UnEs6Class` keep a member a function when the same
scope constructs it or reads its `prototype`: `new C.k`, `C.k.prototype`,
`x instanceof C.k`, `class extends C.k`, and the `C.prototype.k` forms, also
through identifier aliases and IIFE results. `UnPrototypeClass` leaves that
member's assignment after the class. `UnEs6Class` leaves the whole wrapper,
because a `_createClass` descriptor defines a non-enumerable property that an
assignment after the class would not reproduce.

```js
function Hasher() { this.reset(); }
Hasher.prototype.reset = function () { this.n = 0; };
Hasher.Buffer = function () { this.reset(); };
Hasher.Buffer.prototype.reset = function () { this.n = 1; };
// → class Hasher { constructor() { this.reset(); } reset() { this.n = 0; } }
//   Hasher.Buffer = function () { this.reset(); };
//   Hasher.Buffer.prototype.reset = function () { this.n = 1; };
```

Construction the scope cannot see is assumed absent: another module or a
consumer constructing a member of an exported class, a computed key, or a
receiver reached through an instance. Requiring positive evidence instead
(the class does not escape and the member is only called) was measured and
not adopted: static members are mostly the API of exported classes, and on a
large corpus of real sites about 8% of them had that evidence.

Affects: `UnPrototypeClass`, `UnEs6Class`.

Level: every level, like both rules.

### `single_file_esm_consumers`

The consumers of a single input file can load it as an ES module. They are
not in the output, so converting the file from CommonJS to ESM changes its
interface toward them, not only its body:

```js
module.exports = format; // consumer: require("./format")(v)
export default format;   // the consumer's require fails, or returns the
                         // module namespace object, so the call throws
```

A consumer's `require` of an ES module either fails or returns the module
namespace object instead of `module.exports`, and the ESM loader resolves and
evaluates the file differently from `require`. Nothing inside the file shows
how its consumers load it.

Unpack mode does not rely on this. It recovers a bundle's modules together,
so every consumer the bundle contains is converted with its provider, and an
unpacked module still calls the bundler runtime (`require.r`, `require.d`)
that only `UnEsm` removes.

Affects: whether the `UnEsm` runner converts single-file input at all. Every
assumption below that names `UnEsm` applies to single-file input only where
this one is accepted.

Level: `standard` and above. Below `standard`, single-file input keeps the
module system it was written in. Unpack mode converts at every level without
it.

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

In the transpiler output examined so far and in the private fixtures, no
accessor intercepts a chained export assignment. TypeScript's
`exports.A = exports.B = void 0` (from 3.9) and Babel's
`exports.default = exports.x = value` (6 and 7) write keys that have no
accessor at that point. The only accessors these producers install are
getter-only live re-exports. Babel puts them on other keys. From 4.0,
TypeScript also lists a re-exported name in its `void 0` chain and then
installs the getter on that same key, but the chain runs first and nothing
writes the key afterwards. That is an observation, not a guarantee. The hazard
is accepted rather than proven; the CommonJS wrapper only guarantees that
`module`, `exports`, and `require` are defined.

Affects: `UnAssignmentMerging` (chain splitting with repeated identifier
values or CommonJS receivers) and `UnEsm` (every `exports.x = v` to `export`
recovery, whole-chain recovery, and property-storage recovery). Which chains
and receivers each accepts is documented in `rules/un_assignment_merging.rs`
and `rules/un_esm/export_storage.rs`.

Level: `UnAssignmentMerging` relies on it at every level. `UnEsm` relies on it
wherever it runs: `standard` and above for single-file input
(`single_file_esm_consumers`), and every level in unpack mode.

### `import_hoisting_eagerness`

Converting a CommonJS `require()` into an ESM `import` moves the provider's
evaluation ahead of every consumer statement: imports are hoisted and all
dependencies evaluate before the importing module's body. In CommonJS, a
provider executes at its `require` call site, interleaved with the consumer's
own statements. Every wakaru CommonJS recovery shares this deviation; it is
observable whenever a later provider's side effects (a global write, an
installed getter or setter) change what an earlier consumer statement — such as
an `Object.assign` copy — reads. Relative provider order is preserved only
among requires that become imports.

ESM has no `require`, so a CommonJS module that would become ESM with a
`require` call left in it stays CommonJS instead: one the barrier below keeps
in place, a called require, one in a function, or one in a conditional
position. Webpack loader calls whose module id no input links are the
exception: a numeric id (`require(17)`), and a string id (`require("q1tI")`)
that the detector saw the factory pass to its loader. They fail in CommonJS
too, so they stay calls in ESM output (`runtime_residual` reports them). From
Phase 1 to the printed output, unpack writes a kept string id as a number
literal that only prints as the string (`kept_require::kept_string_module_id`:
value `-1`, `raw` the quoted id), so every rule handles the call exactly as an
absent numeric id: `UnEsm` does not import it, `UnWebpackInterop` still folds
a `require.n` getter around it, and JSX through that getter is recovered.
Before that check `UnEsm` converts the module a second time, since some
shapes convert only once it is ESM (`export default require("x")()`). In
unpack it leaves two shapes for later stages: before the first barrier
below, a top-level `var` initialized by an imported function's
`helper(require("./p"))` (`cross_module_interop_import`); and, in any
position, a lowered `import()` around an imported helper
(`Promise.resolve().then(() => helper(require("./p")))`,
`lowered_dynamic_import_source_semantics`). The first loads its provider as
an import, so after a barrier it keeps the module CommonJS like any other
require. When the facts do not prove the imported function an
interop helper, the call stays and only its require becomes an import of the
value it returned (`import_unproven_helper_requires`; a default import for
`module.exports = v` or a provider that stays CommonJS, a namespace for an
ES module, and a default import for a provider outside the input). A require
the stages leave stays in the ESM output, and the
`commonjs_require_unrecovered` warning reports it. A single file has no later
stage, so there these shapes keep the module CommonJS too. Neither has raw
unpack output, nor the late ESM recovery that ends unpack's Phase 2: it runs
after these stages, which found the module still CommonJS. Of the requires
`UnEsm` leaves to later stages, only a webpack lazy load is gone by then,
because its restore reads a module of either format; Phase 1 predicts that
recovery for the module facts with the same assumption.

Source code and minifiers also leave a require inside an expression of any
module: `module.exports = { a: require("./a") }` as written, or a bundled
interop helper's call that Terser inlined into its one use,
`(0, h(require("./x")).default)(Error)` (producer `@babel/core@7.18.13` with
plugin-transform-runtime, webpack@5.111.1 + Terser). When the first
conversion leaves a require behind in a module that is not webpack harmony
output, `UnEsm` converts it again from the start, and this time a require (or
a call of a name around one, `h(require("./x"))`) that its top-level
statement evaluates before any other effect gets a `const` binding right
before the statement (`un_esm/leading_requires.rs`). Only literals, names,
member reads, function expressions, and other moved requires may come before
it, so the binding runs it at the point it ran before; it then becomes an
import like a top-level `var x = require(p)`. A plain require moves only when
its value is passed on (a property value, an array element, a call argument);
one that is called, has a member called, or is assigned keeps its own
handling, and the module stays CommonJS when one remains.

UnEsm limits the deviation with a barrier (`rules/un_esm/hoist_barrier.rs`).
After the first top-level statement that a later provider can observe, every
top-level `require` stays a call in place, so the module stays CommonJS. The
barriers are:

- a write to a global: `window.fetch = spy`, `process.env.X = v`, `g = 1`;
- a string `require` that stays a call: one whose result is called, in an
  expression statement or a declaration (`require("dotenv").config()`,
  `var env = require("dotenv").config()`), or any require inside `if`, a
  loop, or `try`;
- an expression statement that calls into a required module and discards
  the result: `polyfill.install();`.

So `require("dotenv").config(); const db = require("./db")` keeps both
requires in order. Other statements are not barriers: declarations,
function and class definitions, export plumbing, calls whose result is kept
(`var x = lib.make()`), and calls of local functions. A required value read
as an argument (`f(require("x").default)`) becomes an import, so it does not
count as a require that stays a call.

The barrier sees effects only through these shapes, so some observable
effects do not stop hoisting: a global written by a local helper
(`setup()`), a write through a required binding (`cfg.debug = true`), a
call of a destructured require binding (`var { install } = require("p");
install()`), and `Object.defineProperty(window, ...)`. webpack's module concatenation (producer
`webpack@5.111.1` `concatenateModules`) is the main source of mid-body
requires: an inner module's code runs before the next inner module's
external require.

A webpack harmony module (one that calls `require.r(exports)` or
`require.d(exports, …)`) had ESM source, so its top-level loads came from
imports, and webpack emitted them at the top of the factory in import order.
When the first conversion leaves a require behind, `UnEsm` converts such a
module again with two changes. A require that stays a call is no barrier
there: Terser inlines a single-use import binding into its use
(`r(100).A.make([…])`, `{ ...r(200) }`, `(0, n(300).Z)(…)`, producer
`webpack@5.111.1` + Terser) but keeps the order of calls. And each such
inlined load in a position that runs on every evaluation of the module body
gets a `const` binding before its statement, which becomes an import. A
global write or a discarded call stays a barrier, because only module
concatenation puts one before a load. A require in a function or a
conditional position was written by the source and keeps the module
CommonJS. The assumption: an unconditional top-level require in a harmony
module came from an import, so a source that wrote one itself (an ES module
calling `require("x").init()` at the top level, which webpack accepts) has
its call moved ahead of the module body like an import's.

When a harmony module stays CommonJS, `UnEsm` writes the definitions the
runtime calls made: `require.d` getters become `Object.defineProperty(exports,
"name", { enumerable: true, get() { return x; } })`, as a method so Node's
`cjs-module-lexer` finds the name for an ES module importer, and
`require.r(exports)` becomes the `__esModule` definition.

Affects: `UnEsm` require conversion before the first barrier (including
whole-chain recovery of `exports.a = exports.b = require("x")`),
every fact-consuming recovery that imports a proven provider, and the esbuild
unpacker's writer relocation, which moves a top-level state writer into the
module that owns the state.

Level: wherever CommonJS becomes ESM: `standard` and above for single-file
input (`single_file_esm_consumers`), and every level in unpack mode. This is
inherent to emitting ESM from CommonJS.

### `exports_object_closed_to_consumers`

Requiring a CommonJS module returns its `exports` object, an ordinary object.
The code that requires it can add, overwrite, or delete properties on it, for
example to stub an export or to register a plugin. A method call through it,
`lib.fn()`, runs with that object as `this`, so the callee can write to it
too. Once the module is recovered as ESM, a consumer gets its module
namespace object instead (`import * as lib`, or Node's `require()` of an ES
module). The namespace is not extensible, its properties cannot be assigned
or deleted from outside, and it is the `this` of `lib.fn()`. A consumer's
write throws a TypeError in strict code and does nothing in sloppy code. A
write through `this` in the recovered module always throws, because ES
modules are strict:

```js
// provider, recovered as ESM
exports.Type = function Type(tag) { this.tag = tag; };
// a consumer outside the input
const lib = require("./provider.js");
lib.extra = 1;  // CommonJS: adds a property. ESM: TypeError in strict code
lib.Type("x");  // CommonJS: writes `tag` onto the exports object. ESM: TypeError
```

wakaru assumes that code outside the input does not write to the exports
object of a module it recovers as ESM. That code is invisible to it: a single
file, or the entry of an unpacked bundle, is used by consumers that are not
part of the input. Keeping every such module CommonJS would give up ESM
recovery for every module that has an export, so the deviation is accepted.
Calling a constructor-style export without `new`, as on the last line above,
is a misuse that reaches it: in the CommonJS original the call wrote onto the
exports object, and in the recovered module it throws at that first write.
`call_receiver_independence` covers the receivers that lowering adds or
removes; this assumption covers the receiver of a consumer's own `lib.fn()`.

Inside the input, unpack sees every consumer, and this assumption does not
cover them: a provider that a consumer writes to stays CommonJS, unless its
only export is one `module.exports = v` that every write lands on, where
`export default v` holds the same object
([fact-system.md](fact-system.md#shape)). A write unpack
does not follow (inside a function in another module, or one it cannot
identify) is a known gap (see [unpacking.md](unpacking.md#known-gaps)).

Affects: `UnEsm` (every module it recovers as ESM), and the unpack recoveries
that import a provider as a namespace or decompose one.

Level: wherever CommonJS becomes ESM: `standard` and above for single-file
input (`single_file_esm_consumers`), and every level in unpack mode. This is
inherent to emitting ESM from CommonJS.

### `commonjs_export_mirror_coverage`

Babel, sucrase, and TypeScript (for aliases and reassigned functions) keep an
export's value in a module-level local and copy it into the `exports`
property after every write:

```js
let count = exports.count = 0;
function inc() { exports.count = count = count + 1; }
[a, b] = [b, a]; exports.a = a, exports.b = b;
```

When every write of the property copies the local, and every write of the
local is copied in the same statement or in a following statement separated
only by other copies, the property equals the local wherever it can be
observed. `UnEsm` then exports the local live (`export { count }`), drops the
copies, and replaces reads of the property with the local.

Two gaps remain. Between a write of the local and a copy in a following
statement, code that runs inside the write itself (an iterator during array
destructuring, a getter on the right-hand side) could read the stale property.
And a read before a lexical local is initialized returns `undefined` from the
property but throws on the local; a top-level read before the declaration
falls back to property storage, but a read inside a function that runs early
throws in the recovered module as it did in the ESM source. Compiler output
reads neither way. A write of the local without a copy, which hand-written
CommonJS uses to keep the property behind the local, fails the
classification and is not recovered this way.

Affects: `UnEsm` (mirror-storage recovery) and `UnAssignmentMerging`, which
keeps a chain that writes both an `exports` property and a resolved local
whole so the copy stays visible.

Level: wherever `UnEsm` runs: `standard` and above for single-file input
(`single_file_esm_consumers`), and every level in unpack mode.

### `cocos_registration_frame`

Cocos Creator 2.x frames every project script with
`cc._RF.push(module, uuid, script)` and `cc._RF.pop()` at the top level of its
factory. The engine's `push` records `module` and the current
`module.exports`; `pop` replaces `module.exports` with the script's component
class only when the recorded object is still in place and has no enumerable
key. Neither reads or writes an `exports` property, so the `module` argument
does not make an `exports.x` access observe a value the module did not write.
The engine's frame behaves this way from 2.0 through 2.4.

Only a direct top-level `push` whose first argument is the free `module`, and
that a later direct top-level `pop` closes, counts as the frame. Any other use
of `module` is still an escape of the export surface.

The recovered ESM module keeps the frame, including its `module` argument, so
it runs only where something defines `module`, as before this assumption. The
rules that rely on it act only on a module that writes an `exports` property,
which leaves an enumerable key at `pop`, so `pop` returns without replacing
`module.exports` in the original as well. The cross-module fact that requiring
a module returns its `exports` object skips the argument too: a consumer reads
it only for a provider whose default export came from an `exports.default`
write, so another script's `require(...).default` becomes the default import.

The engine also installs tslib's helpers as globals (`window.__extends`,
`window.__decorate`, ...). The editor's compiled scripts declare no helpers
of their own (producer cocos-creator@2.4.15 web-mobile), so a framed script
calls them as free identifiers. In a module that has a closed frame, a free `__extends`
or `__decorate` that the module never writes is that helper.

Affects: `UnEnum` (exported enum folds beside the frame), `UnEsm`
(export-storage classification ignores the frame's `module` argument), and
`UnEs6Class` (the free `__extends` proves TypeScript default inheritance, and
the free `__decorate` lets a component wrapper's decorator calls move out of
it; see [helper detection](helper-detection.md#typescript-decorated-wrappers)).

Level: wherever those rules run; `UnEsm` at `standard` and above for
single-file input (`single_file_esm_consumers`), and at every level in unpack
mode.

### `typescript_helper_guard`

TypeScript emits each helper it inlines into CommonJS output as a guard
over a fallback:

```js
var __exportStar = (this && this.__exportStar) || function (m, exports) { /* … */ };
```

At the top level of a module (outside every non-arrow function and class
body), `this` is the module's `exports` object when a CommonJS loader or a
bundler runs the body, `undefined` in an ES module or a strict call without
a receiver, and the global object in a sloppy call without one. wakaru
assumes the guard is falsy there, so the fallback is what runs: the
`exports` object has a `__name` property only when the module writes it,
and a global `__name` is the same tslib helper that a runtime installed
(tslib's browser build, or the Cocos Creator engine as in
`cocos_registration_frame`), which does what the fallback does. A minifier
keeps the shape under another binding name, or inlines a helper used once
at its call site: `(this && this.__importDefault || function (e) { … })(x)`.

The property name proves nothing by itself. Each rule that relies on the
guard also matches the fallback's body, and `UnEsm`'s top-level `this` check
also requires that the module never accesses that property through
`exports` or `module.exports`. Inside a function or class body `this` is a
receiver whose `__name` property can be any function, so the unpack plan's
use collector reads the guard only where `this` is the module's own.

Affects: TypeScript helper detection (the helper's kind comes from the
fallback), `UnEsm`'s `export *` recovery and its top-level `this` check (the
guard is not a use of the `exports` object), and the unpack plan's use
collector (a required value passed to the helper reaches the fallback, and
the fallback's body decides whether the value is written).

Level: every level; the guard is part of how the helper is recognized.

### `namespace_interop_source_semantics`

rollup's default `output.interop: "default"` builds every external namespace
import with `_interopNamespaceDefault(dep)`. The helper copies the
provider's keys except `default` and then sets `default` to the whole
provider, without checking `__esModule`:

```js
// source
export * as ns from "./dep.js";
// rollup CommonJS
var dep_js = require("./dep.js");
var dep_js__namespace = _interopNamespaceDefault(dep_js);
exports.ns = dep_js__namespace;
```

For a provider compiled from ESM, rollup's CommonJS differs from its own
source: `ns.default` is the whole provider instead of its default export
(`undefined` when it has none). wakaru reads the helper as a wildcard
interop and emits `import * as dep_js__namespace from "./dep.js"`, which
matches the source. Only `ns.default` differs from rollup's CommonJS. When
the provider is real CommonJS, Node's ESM import also gives `default` the
whole `module.exports`, so the two agree on `default`.

This follows the `export *` recovery, which also accepts a difference from
the CommonJS output when the result matches the ESM source the compiler
started from (see [helper-detection.md](helper-detection.md)).

rollup's `interop: "compat"` helper first returns a provider that has a
`default` key unchanged, so its `ns.default` already matches the source for
a provider with a default export; otherwise it behaves like
`_interopNamespaceDefault` and is read the same way.

Affects: helper detection (`is_namespace_default_interop_fn`), so
`UnInteropRequireWildcard` and every rule that treats a wildcard interop
call as a namespace import; and `UnEsm`'s export-star recovery, which can
then recover a star loop that shares the provider binding with the helper.

Level: all levels, like the rest of helper detection.

### `lowered_dynamic_import_source_semantics`

Babel, TypeScript (`esModuleInterop`), swc, sucrase, and rollup
(`dynamicImportInCjs: false`) lower `import(x)` to a wildcard interop of a
`require` inside a promise callback:

```js
// source
const ns = await import("./dep.js");
// TypeScript CommonJS
const ns = await Promise.resolve().then(() => __importStar(require("./dep.js")));
```

wakaru emits `import("./dep.js")`. The result follows ESM semantics where
the CommonJS output differs from its own source:

- The provider loads through the ESM loader: ESM resolution, as for every
  import `UnEsm` emits, and asynchronous evaluation instead of a synchronous
  `require` inside the callback.
- For a CommonJS provider, `ns.default` is the whole `module.exports` in
  both forms, but named keys come from Node's cjs-module-lexer instead of a
  copy of the provider's own keys. A name the lexer cannot detect reads
  `undefined`.
- TypeScript before 4.5 and sucrase evaluate a non-literal specifier inside
  the callback; `import()` evaluates it at the call, as the source did.

The callback shape is compiler output for `import()`, and the result
matches the source the compiler started from, as for
`namespace_interop_source_semantics`. Keeping the lowered form leaves a
`require` that an ESM module cannot call.

Affects: `UnInteropRequireWildcard` when `UnEsm` runs it on a module it
converts, and the Phase 2 pass `run_cross_module_lowered_dynamic_imports`
for a helper imported from another module of a bundle.

Level: wherever `UnEsm` runs: `standard` and above for single-file input
(`single_file_esm_consumers`), and every level in unpack mode, where the
Phase 2 pass also runs.

### `webpack_lazy_import_source_semantics`

webpack and Rspack compile `import(x)` of another module of the bundle to a
loader that runs after a resolved promise or after the chunk loads that hold
`x`:

```js
// source
const ns = await import("./dep.js");
// webpack 5, dep.js an ES module in another chunk
const ns = await __webpack_require__.e(240).then(() => __webpack_require__(240));
// webpack 5, dep.js CommonJS in the same chunk
const ns = await Promise.resolve().then(() => __webpack_require__.t(240, 19));
```

In unpack mode, once the module id resolves to a module of the output,
wakaru emits `import("./dep.js")`, or
`import("./dep.js").then((ns) => ns.default)` when the loader's value is the
default export of the recovered module. The form follows from the loader and
the provider's facts, as for a kept `require`:

- The plain loader returns the exports object. That is the namespace of an
  ES module or of a provider marked `__esModule`, and the default of a
  `module.exports = v` provider recovered as `export default v` or of a
  provider that stays CommonJS.
- `require.t(id, mode)` returns a new object whose `default` is the exports
  object: the namespace of a provider whose default is that object. With
  `mode & 4` and a marked provider it returns the exports object, as the
  plain loader does.
- Any other combination keeps the loader: a fake namespace of a provider
  recovered with named exports only, or of a marked provider without
  `mode & 4`.

The result follows ESM semantics where the bundle differs:

- The chunk loads are dropped. Every chunk is part of the unpack output, and
  `import()` loads the provider together with its own imports.
- The provider loads through the ESM loader, asynchronously, as in
  `lowered_dynamic_import_source_semantics`.
- With `mode & 2`, webpack copies every key of a CommonJS provider's exports
  object onto the fake namespace. `import()` of a provider recovered as
  `export default v` has only `default`, so a named read gives `undefined`.
  Node's `import()` of the CommonJS source gives the names cjs-module-lexer
  detects, so the source itself differs from the bundle here.
- With `mode & 16`, webpack returns a thenable exports object unchanged;
  `import()` resolves to the namespace.

The loader shape is bundler output for `import()`, and the result matches
the source the bundler started from. Keeping the loader leaves `require`
calls that the split output does not define.

Affects: the Phase 2 pass `run_cross_module_webpack_lazy_imports`, and the
kept-require classification that lets a module with such a loader become an
ES module in Phase 1. Turbopack async loader modules, which unpack writes as
`Promise.resolve().then(() => require(target))`, take the same path.

Level: every level, in unpack mode only.

### `esbuild_lazy_import_source_semantics`

esbuild without code splitting bundles an ES module that is loaded with
`import()` as a lazy initializer and a namespace object, and lowers the
`import()` to a call that runs the initializer and returns the namespace:

```js
// source
const ns = await import("./dep.js");
// esbuild --bundle
var dep_exports = {};
__export(dep_exports, { default: () => dep_default });
var dep_default;
var init_dep = __esm({ "dep.js"() { dep_default = 1; } });
const ns = await Promise.resolve().then(() => (init_dep(), dep_exports));
```

A template `import()` builds a `__glob` map whose values are the same
loaders. When the initializer and the namespace object are referenced only
by such loaders, and the module's declarations only by the module itself,
unpack emits the module as a plain ES module (the initializer body at top
level, exports named by the `__export` map) and each loader as
`import("./dep.js")`.

The module still runs on the first load, as the initializer did. The result
follows ESM semantics where the bundle differs:

- The module loads through the ESM loader: `import()` settles a few
  microtask turns later than the resolved promise did, as in
  `lowered_dynamic_import_source_semantics`.
- A throw while the module runs rejects every later `import()` of it. The
  bundle's initializer cleared itself before running, so a later loader
  returned the namespace of the half-run module instead.
- The namespace is a module namespace object instead of a plain object with
  getters: it has a null prototype and `Symbol.toStringTag`.

A module that is also imported statically keeps its initializer: the
importer runs it eagerly, before any loader.

A `__glob` map whose every entry becomes such an `import()` collapses into
`import()` of the template again (`` import(`./pages/${p}.js`) ``). For a
path among the map's keys, the native `import()` loads the same module. For
any other path the bundle threw `Module not found in bundle` synchronously
from the call; the native `import()` returns a rejected promise instead, or
loads whatever file the path names next to the output. Both are what the
source's `import()` did.

Affects: esbuild unpack (`unpacker/esbuild/lazy_import.rs`,
`unpacker/esbuild/glob_import.rs`), in normal and raw output.

Level: every level, in unpack mode only.

### `relative_require_esm_provider`

In single-file decompilation, `UnEsm` cannot see the provider of
`var dep = require("./dep")`, so it emits `import dep from "./dep"`. That
links against any CommonJS provider, because Node gives a CommonJS module's
`module.exports` as its default, but fails to link against an ESM provider
without a default export.

Babel, TypeScript, swc, sucrase, and esbuild lower `import { x } from
"./dep"` to that plain `require` and `dep.x` reads. A default import reads
`dep.default` or wraps the module in an interop-default helper. So in a
module compiled from ESM, a relative `require` binding with neither is a
named import, and its provider most likely a sibling module compiled from
ESM too. The `UnEsm` runner then emits `import * as dep from
"./dep"`, which matches the source.

The module counts as compiled from ESM when it has a top-level
`__esModule` marker (including rollup's combined `Object.defineProperties`
form), or an esbuild `__toCommonJS` namespace, which has no marker. rollup's
`Symbol.toStringTag` marker on its own does not count. When and how the evidence is collected is documented in
`relative_namespace_import.rs`.

The guess is wrong for a CommonJS provider whose export names Node's
cjs-module-lexer cannot detect, such as `module.exports = make()`: a
namespace import reads `undefined` for those names. Bare specifiers keep
the default import, because packages are the likely place for such
providers. rollup output has no `__esModule` marker unless it exports a
default, so its named imports usually stay default imports.

Affects: the relative namespace rewrite in the `UnEsm` runner
(`relative_namespace_import.rs`), which runs only without module facts.
Unpack mode decides the same edge from provider facts.

Level: `standard` and above.

### `default_object_tail_value`

Next's CommonJS dist files end in a tail that replaces `module.exports` with
the default export, with the named exports copied onto it:

```js
if ((typeof exports.default === "function" || typeof exports.default === "object" && exports.default !== null) && exports.default.__esModule === undefined) {
  Object.defineProperty(exports.default, "__esModule", { value: true });
  Object.assign(exports.default, exports);
  module.exports = exports.default;
}
```

`UnEsm` recovers such a module as ESM with the tail kept on the default
binding. A consumer that stays CommonJS, such as a passthrough
`module.exports = require("./link.js")`, then gets the namespace from
`require`, where it got the default before. Unpack treats the module's
CommonJS value as its default export (`commonjs_value_is_default`), so a kept
`require` reads `.default`. The passthrough's own consumers require the
tailed module directly (barrier 0 retargets a require of a passthrough), so
they read it through its default import too.

The tail does that only when the default is a function or an object at run
time, which is usually not provable: Next's defaults are values such as a
`forwardRef(…)` result. The assumption is that a tailed module's default is a
function or an object unless it is provably a primitive (every value written
to it a literal, a template, `void x`, `undefined`, or a binding initialized
with one). Next adds the tail for components, so a primitive default is not
expected. When the assumption is wrong, the tail left `module.exports` the
exports object, and the rewritten `require(x).default` reads the primitive
instead of that object.

A provably primitive default leaves the module a provider marked
`__esModule` whose value is unknown. A consumer that reads only
`.default` of it reads the default export either way, so
`provider_namespace_repair` gives it the namespace import, and namespace
decomposition then the default import.

Affects: `collect_default_object_tail` in `facts.rs`, read by
`run_kept_require_default_reads` and the provider repairs through
`ProviderInterface`.

Level: every level. Without it the kept `require` reads the namespace, which
is already wrong for every tailed module whose default is a function or an
object.

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

The proof is `TempIsolation` in `rules/binding_facts.rs`. Every rule that can
drop a temp's write passes each rewrite through it at its choke point; a
pattern's own use-count check is an early exit, not a substitute.

`SmartInline` removes generated-looking single-read `const` aliases under a
separate, position-independent proof that the alias source is frozen
(`inline_temp_vars` in `rules/smart_inline.rs`). Aliases with meaningful names
stay as readability policy.

## Declaration-Kind Capture Safety

`VarDeclToLetConst` turns a captured `var` into `let`/`const` only when a
same-scope proof shows that no capture can run before the declaration
completes. The proof and its boundaries are described in
`rules/var_decl_to_let_const_captures.rs`. It is bounded to one scope: it does
not follow object aliases, and it adds no cross-module entry roots, so
`minimal` keeps exported `var`s.

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
break code a direct `eval` evaluates. Binding-oriented rules guard through
`rules/eval_utils.rs`: `DirectEvalAnalyzer` classifies direct eval calls and
their sources, and `js_source_mentions_binding` limits the bail-out to bindings
a known source string mentions (an unknown source blocks all).
`VarDeclToLetConst`, `DeadDecls`, `UnIife`, and `ArrowFunction` follow this
pattern; each rule documents what it checks.

A rule that reads a free name as the global, renames or removes a binding, or
introduces a new binding guards against both constructs at this granularity:

- A `with` statement anywhere in the module skips the rule for the whole
  module (`module_has_with_stmt`). wakaru does not model `with` bodies. A
  rule may skip less when it can tell which rewrites a `with` body could
  observe; `VarDeclToLetConst` keeps only the `var`s inside the body.
- A direct `eval` whose source is unknown blocks the rewrite. A direct `eval`
  whose source is a static string blocks only the names that source mentions,
  the best effort above. For a synthesized reference to a global such as
  `undefined` or `Infinity`, `module_blocks_global_reference(module, name)`
  applies both `with` and `eval` cases. A rule that matches `undefined` read
  as the global uses `module_reads_rebindable_undefined`, which also lets a
  module that spells it only `void 0` through.
- A rule that cannot cheaply name every binding a known source could reach
  uses `has_dynamic_scope_construct` instead, and skips on any `with` or
  direct `eval`. This is the default for a rule that adds the guard: the
  skip covers every name at once, so the rule needs no per-name analysis.
  The one direct `eval` it lets through is a known source that only reads
  CommonJS `require` (`eval("require('crypto')")`, or the
  `eval("quire".replace(/^/, "re"))` form generated code uses to hide a
  `require` call from bundlers), while the module declares no binding named
  `require`; SmartRename never names a binding `require`. Part of a module
  (`node_has_dynamic_scope_construct`) cannot see that declaration, so there
  every direct `eval` counts.
- A rewrite that touches only bundler temps keeps the compiler-temp rule
  above, so `UnWebpackInterop` still inlines webpack's interop getter
  (`__webpack_require__.n`) and namespace cache (`__webpack_require__.t`)
  under a direct `eval`.

Compilers do not emit either construct, so this coarse treatment costs little
recovery on compiled output. A rule in this set that lacks the guard is a bug,
not a documented exception. `UnEsm` also keeps a module with a `with`
statement CommonJS, because ESM is strict code, where `with` is a syntax
error.

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
6. Keep this document to the contract: what the assumption is, a minimal
   counterexample, why it is accepted, its level, and which rules rely on it.
   How a rule matches, guards, or proves a shape belongs in a code comment
   next to that code, named by the assumption so it stays grep-able.
