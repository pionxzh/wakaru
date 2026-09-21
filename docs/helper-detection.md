# Helper Detection Design

> **Before proposing a generic matcher to "reduce the LOC" here, read
> [learnings/helper-detection-pattern-engine.md](learnings/helper-detection-pattern-engine.md).**
> Replacing bespoke detection with a corpus matcher / ast-grep-style DSL /
> skeleton-pattern engine was built, measured against real bundles, and
> reverted: ~93% of detection is marker-based or stateful and can't be
> expressed as a fixed pattern, and the migratable remainder is too small for a
> shared engine to pay off. The size is the cost of the problem, not a missing
> abstraction. Keep matchers bespoke.

Design notes for detecting and restoring transpiler runtime helpers in wakaru.
See [architecture.md](architecture.md) for overall pipeline structure and
[rule-dependency-inventory.md](rule-dependency-inventory.md) for where helper
rules sit in the pipeline ordering.

## Problem

Transpilers (Babel, TypeScript/tslib, SWC) inject runtime helper functions to polyfill modern syntax for older targets. In bundled output, these helpers appear in several forms:

1. **Imported** from a runtime package — `require("@babel/runtime/helpers/interopRequireDefault")` or `import _extends from "@babel/runtime/helpers/extends"`
2. **Inlined** at the top of each module — the function body is copied directly, no import
3. **Hoisted** into a shared webpack module — accessed via numeric `require(42)`, name lost entirely
4. **Minified** — parameter and function names are mangled, but the body structure is preserved

The TS wakaru handles case 1 (match by import path) and case 3 (top level declaration, matching with regex). Rust wakaru should handle all four.

## Approach: match by function body shape

Instead of matching import paths or function names, detect helpers by their **AST structure** (ignoring variable names). This naturally handles all four cases above.

Example — `interopRequireDefault` across transpilers and minifiers:

```js
// Babel 7
function _interopRequireDefault(obj) { return obj && obj.__esModule ? obj : { default: obj }; }
// SWC
function _interopRequireDefault(obj) { return obj && obj.__esModule ? obj : { "default": obj }; }
// Minified
function(e) { return e && e.__esModule ? e : { default: e }; }
```

The essential shape is always: single param, conditional on `__esModule`, returns `{default: param}`. A matcher checks these structural properties and ignores names.

## What we decided NOT to do

These ideas were explored and rejected:

- **Custom IR layer** — SWC's AST is already high-level (has CallExpr, AwaitExpr, YieldExpr, etc.). A second IR would duplicate representation and debugging cost without solving the actual problems. We already do generator-to-async restoration directly on SWC AST in `un_async_await.rs`.

- **CFG hashing / structural fingerprints** — Sounds appealing but fragile in practice. Small codegen/minifier changes scramble naive hashes, and stable canonicalization is the hard part (not storing the graph). Overkill for functions that are typically 1-5 lines.

- **Version auto-detect via runtime strings** — Bundled code often strips version markers, and inlined helpers erase them entirely. Designing around version gating would fail on real-world bundles.

- **Configurable pass graphs / incremental re-analysis** — Premature optimization. The current linear pipeline in `crates/core/src/rules/pipeline.rs` is descriptor-based but still intentionally fixed-order.

## Architecture

Helper recovery is intentionally split into three layers. This gives us more
structure than scattered hand-written tuple checks, without committing to a
general AST pattern DSL.

### Binding-aware matching (`match_context.rs`)

`MatchContext` is used inside helper body-shape matchers when several
identifiers must refer to the same binding. It extracts named slots from
function params or discovered locals, then exposes checks like
`ctx.is_binding(expr, "source")` and `ctx.is_member_of(expr, "source", "default")`.

Use it when matching a helper implementation where shadowing or swapped
operands would produce a false positive. Examples include Babel helpers such as
`_classCallCheck`, `_inherits`, `_possibleConstructorReturn`, and
`_objectWithoutProperties`.

Do not use `MatchContext` as a full AST pattern engine. The surrounding matcher
should still be ordinary Rust over SWC nodes; `MatchContext` exists to make
binding identity explicit and hard to forget.

### Helper lifecycle utilities (`helper_matcher.rs`)

`helper_matcher.rs` contains the low-level binding primitives shared by helper
rules across Babel, TypeScript, webpack, and template helper recovery:

- `BindingKey` and extraction helpers such as `binding_key()`,
  `expr_binding_key()`, and `var_declarator_binding_key()`
- binding-safe predicates such as `ident_matches_binding()`,
  `expr_matches_binding()`, and `member_of_binding()`
- declaration cleanup helpers such as `removable_without_remaining_refs()`,
  `remaining_refs_outside_*()`, `remove_fn_decls_by_binding()`, and
  `remove_var_declarators_by_binding()`

Use these when a rule has already identified helper bindings and needs to track
uses, rewrite call sites, or remove consumed declarations. This keeps the common
scope-sensitive lifecycle code in one place while leaving each rule's semantic
matching local to that rule.

`remove_unused_helper_declarations()` is the shared cleanup entry point for
caller-proven function/variable declarations. It scans references across the
supplied module, computes a stable removable set, then removes declarations in
both module items and nested statement lists (including function bodies).
Mixed variable declarations retain unrelated declarators. Direct exports and
references from sibling scopes keep a helper alive. It returns the removable
set so a caller can apply its import policy separately; it does not remove
imports or prove that arbitrary initializers are safe to discard.

Destructuring, ES6 class, private-field helper, and regenerator cleanup use this
entry point, as does the shared transpiler-helper lifecycle. Rules still own
candidate selection: ES6 class retains its declaration-shape checks and defers
cleanup until all lists are rewritten; destructuring accepts only function
declarations or direct function/arrow initializers for its name-based consumed
helpers. Its sliced-to-array helpers still go through dependency cleanup.
Interop module evaluation, private backing-map initialization, and regenerator
mark bookkeeping remain rule-specific.

Two removal rules hold for every caller. Removal iterates until the removable
set is stable: a helper the module still references stays, and so does every
helper it references, because references inside a kept declaration count.
`remove_helpers_without_remaining_refs` and `removable_without_remaining_refs()`
implement this; a rule-local sweep must not call `remaining_refs_outside_*()`
once and remove the difference, because that skips every candidate declaration
and deletes a dependency of a kept candidate (an esbuild `__defProp` alias under
a kept `__defNormalProp`, a `__values` helper called from a kept `__generator`).
Import cleanup drops only an import that lost its last specifier; a bare
`import "./side.js"` is a side effect the module depends on and is never
removed by helper cleanup.

### Rule-local matching

Rules still own domain-specific shape recognition. For example:

- `transpiler_helper_utils.rs` classifies known Babel/TypeScript helper bodies
  and runtime imports.
- `un_typeof_polyfill.rs` recognizes Babel/SWC `typeof Symbol.iterator`
  polyfills, including the self-redefining cached function form. That form is
  matched against the declaration's `BindingKey`: both the cache assignment
  and recursive call must target the same function binding and the recursive
  argument must be the helper parameter.
- `un_to_consumable_array.rs` recognizes TypeScript `__spreadArray`.
- `un_template_literal.rs` recognizes Babel/SWC/TypeScript tagged-template
  helper calls and cache factories. Detection uses body-shape signals —
  see "Tagged template body shapes" below.
- `un_webpack_interop.rs` recognizes webpack `require.n`, `require.t`, and
  `require.o` helper forms.
- `un_object_spread.rs` recognizes esbuild's mangled `__spreadValues` /
  `__spreadProps` helpers. This detection is **stateful** and stays rule-local
  on purpose: esbuild aliases `Object.defineProperty`,
  `Object.prototype.hasOwnProperty`, etc. into local variables, and the spread
  helpers are matched relative to those module-wide aliases rather than by a
  self-contained body shape. The central scanner's matchers are
  `fn(&Function) -> bool` and must not depend on bundler-specific module state,
  so moving this in would couple the scanner to esbuild internals. This is the
  documented "deliberate exception" the unification proposal anticipated; see
  [learnings/helper-detection-pattern-engine.md](learnings/helper-detection-pattern-engine.md).
- `un_for_of.rs` recognizes Closure Compiler's `$jscomp.makeIterator` call
  locally. It does not freeze the helper function body, which is versioned
  runtime code outside the loop being recovered. Instead it requires the exact
  member name on either an unresolved `$jscomp` runtime or the canonical
  `var $jscomp = $jscomp || {}` namespace bootstrap, then verifies the complete
  adjacent `.next()` loop and whole-module binding non-escape conditions. This remains
  rule-local because the proof is the namespace-plus-consumer shape and no
  helper declaration is removed.

This is deliberate. A helper matcher should encode the smallest semantic shape
that proves the transform is safe, while shared utilities handle binding
identity and declaration lifecycle mechanics.

### Detection (`transpiler_helper_utils.rs`)

The `collect_transpiler_helpers()` function scans module-level declarations (function declarations, function-assigned variables, TypeScript helper imports, and Babel runtime imports) and returns helper identities by running each candidate through a set of shape matchers or matching known runtime package paths.

Pipeline consumers do not call `collect_transpiler_helpers()` directly. `apply_rules()` lazily builds a `LocalHelperContext` the first time a helper rule needs local helper bindings, after earlier syntax normalization rules have run. Later helper rules in the same pipeline range reuse that context instead of rescanning the module. Direct rule tests can still run individual `VisitMut` rules; those rules build a local context for themselves.

```
scan module-level declarations
  → for each function body, run shape matchers
  → for each Babel runtime import, map the import path to a helper kind
  → for each tslib import/require alias, map the raw TS helper kind
  → collect (binding_key, TranspilerHelperKind) pairs
```

Shape matchers are plain functions over SWC nodes. Most are
`fn(&Function) -> bool`; self-referential helpers additionally receive the
declaration's binding identity. They check essential structural elements and
ignore variable names. Writing a new matcher for a new helper is just writing
a new predicate.

`LocalHelperContext` also records TypeScript and `tslib` helper identities. Consumers use those binding identities directly; for example `UnAsyncAwait` matches detected `__awaiter` / `__generator` aliases instead of first renaming aliases to canonical global names. Shared call-site helpers such as `is_helper_callee()` cover local helper bindings, tslib namespace members, and direct `require("tslib").helper` calls.

`UnAsyncAwait` uses the raw TS call-site helpers for known tslib namespace
members and direct `require("tslib").__awaiter` / `.__generator` calls in both
single-file and unpack pipelines. The namespace must match a collected binding,
and `require` must be unresolved; a `with` statement prevents member recovery
because it can override those lookups. The same callee checks drive generator
decoding and rollback, so a mixed alias/member wrapper cannot lose its awaiter
while returning an undecoded generator iterator.

Generator delegation carries the same context into the `__values` decoder,
including cross-module namespace facts. It removes only a one-argument,
non-spread call to a proven helper (or an unresolved canonical name), and
preserves dynamic lookup and reassigned helper bindings.

`UnRegenerator` also consumes modern SWC async runtime namespace facts. A
`require("@swc/helpers/_/_async_to_generator")` binding is a module object;
its `_` member is the helper function. The shared context keeps those identities
separate and also recognizes explicit namespace imports. Recovery requires
one declaration for the namespace binding and only static `_` reads: a second
`var helper = custom` is a replacement even without an assignment expression.
The shared binding-use index exposes this declaration proof separately from
use classification. Replacement, property mutation/deletion, namespace escape,
`with`, or direct `eval` retain the calls. Cleanup removes a helper only after
all references have disappeared, including unsupported calls left behind.

At `standard` and above, `UnEsm` preserves a proven read-only SWC async runtime
require as a namespace import. This keeps its identity through later helper-context
rebuilds and preserves the runtime's named-only export when a call cannot be
recovered. Unsafe namespace uses keep the CommonJS module boundary instead of
inventing an ESM default export or turning a mutable object into an immutable
namespace. This preparation runs after the existing self-require guard and
before ordinary require conversion; it does not broaden helper-package matching.

Helper utilities include `LocalHelperContext::helpers_of_kind()` (filter by kind), `remove_helper_declarations()` (delete the helper function), `helpers_with_remaining_refs()` (check if a helper binding is still referenced elsewhere), and TS cleanup helpers such as `remove_unused_inline_ts_helpers()` / `remove_unused_ts_helper_bindings()`.

`collect_module_facts()` records two helper export channels:

- `helper_exports` for semantic transpiler helpers represented by `TranspilerHelperKind` / public `HelperKind`.
- `ts_helper_exports` for raw TypeScript/tslib helpers such as `__awaiter`, `__generator`, and `__spreadArray`.
- `ts_helper_namespace_factory_exports` for exported zero-argument CommonJS
  wrapper functions whose bodies both register raw tslib helpers and return the
  corresponding namespace object.

Cross-module consumers use both channels. `UnAsyncAwait`, for example, accepts
named helper imports, namespace members, and proven namespace-factory results;
it does not infer helper identity from a `.__awaiter` or `.__generator` property
name alone.

### Restoration

Each helper kind has its own dedicated rule struct (e.g., `UnInteropRequireDefault`, `UnInteropRequireWildcard`, `UnClassCallCheck`). Each rule implements `VisitMut` for focused rule execution and also exposes a cached pipeline entry point that receives `LocalHelperContext`, then rewrites call sites.

For example, `UnInteropRequireDefault`:
- `var _a = _interopRequireDefault(require("a"))` becomes `var _a = require("a")`
- `_a.default` becomes `_a` (at all reference sites), removing exactly one
  interop layer; deeper authored layers are retained
- The helper declaration is removed only when no reference to it survives
  outside the declaration itself. A helper that is the module's own export
  (Babel's runtime `interopRequireDefault` module), re-exported, or aliased
  keeps its declaration

SWC AMD's assignment form `_a = _interopRequireDefault(_a)` — and the modern
external-helper spelling `_a = helper._(_a)`, proven against the exact
`@swc/helpers` import path — is recovered under a fail-closed proof: the
assignment must be the binding's unconditional top-level first use, with no
other writes and no earlier module evaluation able to observe the binding.
Rejected recoveries keep the wrapper call and helper in place. The full gate
inventory lives in `crates/core/src/rules/un_interop_require_default.rs` and
its tests.

`UnEsm` also handles Babel's equivalent helper body when it is inlined directly
at a `require()` site:

```js
var wrapped = (temp = require("a")) && temp.__esModule
  ? temp
  : { default: temp };
use(wrapped.default);
```

It recovers a default import only when every use of `wrapped` is a read through
`.default` and the assigned `temp` is a hoisted `var` or an earlier
uninitialized `let` used exclusively by that exact helper expression. A later
lexical declaration bails out so removing the assignment cannot erase a TDZ
failure. Bare, computed, written, or otherwise escaping wrapper uses also bail
out. This matcher stays rule-local because the proof combines
the producer shape with module-wide binding-use facts; helper names alone carry
no provenance. Calling a recovered `.default` binding also relies on the
`call_receiver_independence` assumption documented in
[rewrite-assumptions.md](rewrite-assumptions.md).

### Compressed TypeScript import-star factories

TypeScript 5.9's inline `__importStar` uses an `ownKeys` factory. Terser can
lift its local variable and turn the factory IIFE into a sequence expression.
Detection accepts that form only when its prefix assigns a function to a
single uninitialized `var` used exclusively by the helper declaration. An
external read/write, lexical declaration, extra prefix effect, `eval`, or
`with` prevents recognition, so helper cleanup cannot erase observable
initialization. This module-wide proof belongs in the helper collector; the
per-expression body matcher cannot establish that the lifted variable is private.

### Where it runs in the pipeline

Helper detection and restoration runs within **Stage 2** of the `apply_rules()` pipeline, after Stage 1 syntax normalization. Stage 1 rules like `UnIndirectCall` and `UnBracketNotation` must run first to normalize patterns like `(0, x.default)()` and `["default"]` before helper detection can match reliably.

## Transpiler helper coverage

Priority targets, roughly ordered by real-world frequency:

| Helper | Babel | tslib | SWC | Semantics |
|---|---|---|---|---|
| `interopRequireDefault` | `_interopRequireDefault` | — | `_interop_require_default` | Unwrap default import |
| `interopRequireWildcard` | `_interopRequireWildcard` | — | `_interop_require_wildcard` | Unwrap namespace import |
| `extends` | `_extends` | `__assign` | `_extends` | Object.assign polyfill |
| `classCallCheck` | `_classCallCheck` | — | `_class_call_check` | `if (!(this instanceof X)) throw` guard |
| `createClass` | `_createClass` | — | `_create_class` | defineProperties for class methods |
| `slicedToArray` | `_slicedToArray` | `__read` | `_sliced_to_array` | Destructuring arrays from iterables |
| `toConsumableArray` | `_toConsumableArray` | `__spreadArray` | `_to_consumable_array` | `[...arr]` polyfill |
| `arrayLikeToArray` | `_arrayLikeToArray` | — | `_array_like_to_array` | Array-rest copy sub-helper, including mangled declarations |
| `objectWithoutProperties` | `_objectWithoutProperties` | `__rest` | `_object_without_properties` | `const {a, ...rest} = obj` |
| `typeof` | `_typeof` | — | `_type_of` | Native `typeof`, including self-caching declarations |
| `asyncToGenerator` | `_asyncToGenerator` | `__awaiter` + `__generator` | `_async_to_generator` | async/await (already handled in `un_async_await.rs`) |
| `asyncIterator` | `_asyncIterator` (+ `AsyncFromSyncIterator` dependency) | `__asyncValues` | `_async_iterator` | `for await` adapter; the loop protocol is recovered by `un_for_await.rs`. esbuild's `__forAwait` (+ `__knownSymbol`) is matched by shape inside that rule |

esbuild helpers (`__commonJS`, `__esm`, `__toESM`, `__toCommonJS`) are bundler-level and already handled in the unpacker, not here.

`UnDestructuring` accepts a mangled `arrayLikeToArray` declaration only when
its body proves the complete helper contract: the canonical null/length guard,
an unresolved `Array(length)` allocation, a bounded element-for-element copy
loop, and return of that exact allocation. Near-matches with a different guard,
source index, output binding, or shadowed `Array` remain untouched.

`UnSlicedToArray` removes iterator materialization only together with a
proven destructuring group. At `standard` and above, an adjacent return made
of ordered `temp[0]` through `temp[N - 1]` reads and eager binary operators
can recover a complete pattern with fresh, collision-free element bindings.
After all `N` reads, the expression may also contain literal or identifier
leaves, such as `temp[0] + temp[1] + other`. Leading and interleaved leaves
remain outside the current shared grammar; this does not mean every such
expression is unsafe. TypeScript's ordinary iterable `__read` path materializes
a fresh array, but Babel/SWC sliced helpers can return the input array itself.
For those fast paths, moving element reads into an earlier pattern can matter:
with `items = [1, 2]`, if reading the global `other` sets `items[0] = 10` and
returns `0`, `other + temp[0] + temp[1]` returns `12` when `temp` aliases
`items`. Reading `[a, b] = items` before that return instead produces `3`.
The position of `other` within the return did not change; the element reads
moved ahead of it. Widening the shared grammar therefore needs to account for
helper fast paths, rather than infer equivalence from `__read` alone.
This path accepts direct two-argument helper calls, not `_maybeArrayLike`
wrappers that pass a helper as an argument. The return must account for every
use of the temporary. Consumed bindings must each have one declaration; the
helper must remain stable, and namespace helpers must have only static member reads.
Only hoisted `var` declarations without initializers may intervene. Calls,
short-circuit expressions, escaped arrays, dynamic scope, and incomplete or
reordered reads retain their helper call; `minimal` retains this compressed
form too. A later rule is not assumed to reconstruct a pattern. Zero-length
calls also retain their binding if their result is still used. Groups with default elements are the one hand-off:
`UnSlicedToArray` skips them, and `UnDestructuring` drops the proven helper
call when the rebuilt top-level pattern covers exactly the `N` elements the
call materializes and has no rest element. `UnParameters2` then folds that
pattern into the parameter as before. Equal `N` establishes the group shape,
not default/iterator evaluation order. This recovery uses the
[`iterator_materialization_independence`](rewrite-assumptions.md#iterator_materialization_independence)
assumption at `standard` and above; `minimal` retains materialization for these
default groups.

Helper origin/binding identity alone is not enough to remove the call:
`UnDestructuring` filters these candidates with
`BindingUseIndex::collect_direct_write_bindings`, including writes in nested
functions. A write to a different, shadowed binding does not disqualify the
helper. This is a consumer requirement; other recognized helpers may legitimately
redefine themselves, so the shared origin map is not globally filtered.

`UnSlicedToArray` also restores callback-local destructuring when a proven
helper is applied directly to one callback parameter. It accepts either an
unconditional leading declaration such as
`const value = sliced(entry, 2)[1]` or a direct equality comparison of that
indexed result. The parameter must have no other uses, the limit and index must
be bounded literals, and the recovered array pattern retains trailing elisions
so it consumes exactly the helper's requested number of iterator values.
Conditional/deferred access, `arguments`, direct `eval`, `with`, and minimal
rewrite mode all preserve the lowered form.

### TypeScript default inheritance

`UnEs6Class` can consume the canonical `base !== null &&
base.apply(this, arguments) || this` default constructor at `standard` and
above, then recover `base.prototype.method.call(this, ...)` and static
`base.method.call(this, ...)` as native `super` calls. The source-recovery
tradeoff is named
[`native_class_inheritance`](rewrite-assumptions.md#native_class_inheritance).

The anonymous, synchronous wrapper must have one ordinary superclass argument.
The extends call must use the exact resolved constructor and wrapper parameter,
with a proven TypeScript helper or a tslib namespace. Helper aliases,
namespaces, the superclass parameter and the constructor require single,
stable declarations; namespaces must have only static member reads. Direct
`eval` or `with` disables this new path. The module's use index is shared with
nested visitors so writes outside the immediate wrapper still disqualify it.

Only the exact parameterless, single-return default constructor is consumed.
Superclass method calls require `this` as their first non-spread argument and
a static method name. Lexical arrows share the method's `super`; ordinary
nested functions and classes do not. Remaining superclass captures, references
to the removed inner constructor binding, custom constructor bodies, `.apply`
method calls and dynamic method names retain the wrapper. The constructor
reference exception is a static, synchronous method with exactly one statement:
`return new C(...)`. Its sole reference to the original constructor is rebound
to the recovered class using resolver identity. A differently named outer
binding is supported only when the new name cannot collide with method
parameters; reads of the enclosing class variable also prevent recovery.
Deferred factories, constructor writes, and other inner-constructor uses remain
unsupported. This is not general
superclass recovery. Real TypeScript 5.9.3 and Terser 5.51.2 outputs are checked
in under `tests/fixtures/tslib-inheritance/`. The compressed variants use the
same module-mode Terser settings as the matrix. If compression lifts the
helper's `extendStatics` factory into a sequence, the TypeScript helper collector
checks both function bodies and the resolved factory call, then applies the
same module-wide private-local proof as the import-star factory. An external
reference, initialized/redeclared/lexical local, extra prefix effect, `eval`,
or `with` prevents recognition. Cleanup consumes the sequence only through
that proven helper identity.

After class recovery, a named tslib `__extends` import (including aliases) is
removed only if it was referenced before this pass and has no remaining uses.
Other specifiers, re-exports, and originally-unused imports are retained.
Dynamic lookup disables this cleanup. Removing the last specifier leaves
`import "tslib"` to preserve module evaluation; namespace imports are unchanged.
This is consumed-helper cleanup, not a general unused-import optimization.

A retained helper call must also survive the later `UnPrototypeClass` pass.
An unknown interleaved call receiving the constructor blocks prototype-method
folding for hoisted functions as well as variable-initialized functions. This
prevents a leftover `__extends(Child, Parent)` from trying to replace a native
class's non-writable `prototype`. Existing recovery across property-only calls
such as `Object.defineProperty(Child.prototype, ...)` remains unchanged.

### Private-field backing-map lifetime

`UnClassFields` promotes a backing WeakMap only when one class owns it and
its initialization has a stable lifetime. It must have exactly one allocation:
either before the class, or immediately after a class without definition-time
executable members. Owners include top-level class declarations, default-export
classes, single-declarator class expressions, and class assignments to resolved
local bindings (the result of splitting tsc's class-expression sequences).
For class expressions, the owner binding is the outer target, not an optional
class self name. Local export lists, empty statements, and a default export
reading that exact owner binding may intervene; arbitrary identifiers, calls,
and other executable statements may not. Earlier construction, later resets
(including comma expressions), escaping maps, and unsupported map uses keep the lowered
form. These are proof requirements, not rewrite assumptions.

Get/set calls accept resolver-proven tslib named aliases, namespaces, direct
`require("tslib")` members, and inline marker/body matches. The consumer still
requires `this`, field mode `"f"`, exact arity, and no spread arguments. Written
helper bindings or namespaces and dynamic `with` lookup prevent promotion.

### Tagged template body shapes

`taggedTemplateLiteral` detection uses signal-based matching on a 2-param
function body. Three transpiler variants are recognized:

| Variant | Signals required | Body pattern |
|---|---|---|
| Babel spec | `slice_copy` + `freeze_define_raw` | `Object.freeze(Object.defineProperties(strings, {raw: {value: Object.freeze(raws)}}))` |
| Babel loose | `slice_copy` + `raw_assignment` | `strings.raw = raws` (simple property assignment) |
| TypeScript | `define_property_raw` | `Object.defineProperty(strings, "raw", {value: raws})` |

`slice_copy` matches `strings.slice(0)` (the fallback copy when `raws` is
absent). `raw_assignment` matches `strings.raw = raws` as an `AssignExpr`.
`define_property_raw` matches `Object.defineProperty(strings, "raw", ...)`.

The spec variant uses `Object.freeze` and `Object.defineProperties` as global
anchors, making it reliably detectable even when mangled. The loose variant
has no global anchors — it's detected only by the `slice(0)` + `.raw =`
combination on the two params. The esbuild variant aliases `Object.freeze` and
`Object.defineProperty` into local variables, which breaks global-anchored
matching in raw minified output. `UnBuiltinAliases` runs after early
declaration splitting and before helper-dependent structural recovery, so
module-scope builtin aliases are normalized back to global member reads before
the central body-shape scanner sees the helper.

## Handling version drift

Babel helpers do change across versions (bug fixes, spec compliance, browser capability changes). The solution is **relaxed matching** — check the essential semantic structure, not exact AST equality.

For `interopRequireDefault`, the essential structure has been stable for years because it's defined by the ES module spec: "if `__esModule`, return as-is; otherwise wrap in `{default: ...}`". If a future version fundamentally changes what a helper *does*, it's a new helper and gets a new matcher.

In practice, most variation across versions is:
- Different conditional forms (ternary vs if/else)
- Property access style (`.default` vs `["default"]`)
- Extra `Object.defineProperty` for non-configurable exports
- Added null checks

A good matcher checks for the presence of `__esModule` and `default` in the right structural positions, and tolerates everything else.
