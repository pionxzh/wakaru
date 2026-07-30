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
(call sites rewritten from `.default`), and `UnEsm` (default interop recovery).

Level: receiver-changing `UnIndirectCall` and `UnEsm` forms require `standard`
or above. Explicit transpiler-helper recovery in `UnInteropRequireDefault`
applies whenever that helper is recognized.

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

Affects: `UnOptionalChaining` (loose null-check forms, including cloned
restored-listener normalization during Angular recovery),
`UnNullishCoalescing` (loose null-check forms).

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
and every fact-consuming recovery that imports a proven provider.

Level: all levels. This is inherent to emitting ESM from CommonJS.

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

Candidate declarations still participate in reference counting. This prevents
two independently removable aliases from forming a replacement chain whose
intermediate binding is deleted before a non-recursive substitution uses it.

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
