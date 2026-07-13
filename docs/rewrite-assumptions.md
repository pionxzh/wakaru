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
rewrite. The unresolved global `undefined` is the only global exception.
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
