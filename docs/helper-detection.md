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
- declaration cleanup helpers such as `remaining_refs_outside_*()`,
  `remove_fn_decls_by_binding()`, and `remove_var_declarators_by_binding()`

Use these when a rule has already identified helper bindings and needs to track
uses, rewrite call sites, or remove consumed declarations. This keeps the common
scope-sensitive lifecycle code in one place while leaving each rule's semantic
matching local to that rule.

### Rule-local matching

Rules still own domain-specific shape recognition. For example:

- `transpiler_helper_utils.rs` classifies known Babel/TypeScript helper bodies
  and runtime imports.
- `un_typeof_polyfill.rs` recognizes TypeScript `typeof Symbol.iterator`
  polyfills.
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

Shape matchers are plain functions: `fn(&Function) -> bool`. They check essential structural elements and ignore variable names. Writing a new matcher for a new helper is just writing a new predicate.

`LocalHelperContext` also records TypeScript and `tslib` helper identities. Consumers use those binding identities directly; for example `UnAsyncAwait` matches detected `__awaiter` / `__generator` aliases instead of first renaming aliases to canonical global names. Shared call-site helpers such as `is_helper_callee()` cover local helper bindings, tslib namespace members, and direct `require("tslib").helper` calls.

Helper utilities include `LocalHelperContext::helpers_of_kind()` (filter by kind), `remove_helper_declarations()` (delete the helper function), `helpers_with_remaining_refs()` (check if a helper binding is still referenced elsewhere), and TS cleanup helpers such as `remove_unused_inline_ts_helpers()` / `remove_unused_ts_helper_bindings()`.

`collect_module_facts()` records two helper export channels:

- `helper_exports` for semantic transpiler helpers represented by `TranspilerHelperKind` / public `HelperKind`.
- `ts_helper_exports` for raw TypeScript/tslib helpers such as `__awaiter`, `__generator`, and `__spreadArray`.

### Restoration

Each helper kind has its own dedicated rule struct (e.g., `UnInteropRequireDefault`, `UnInteropRequireWildcard`, `UnClassCallCheck`). Each rule implements `VisitMut` for focused rule execution and also exposes a cached pipeline entry point that receives `LocalHelperContext`, then rewrites call sites.

For example, `UnInteropRequireDefault`:
- `var _a = _interopRequireDefault(require("a"))` becomes `var _a = require("a")`
- `_a.default` becomes `_a` (at all reference sites)
- The helper function declaration is removed

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
| `objectWithoutProperties` | `_objectWithoutProperties` | `__rest` | `_object_without_properties` | `const {a, ...rest} = obj` |
| `asyncToGenerator` | `_asyncToGenerator` | `__awaiter` + `__generator` | `_async_to_generator` | async/await (already handled in `un_async_await.rs`) |

esbuild helpers (`__commonJS`, `__esm`, `__toESM`, `__toCommonJS`) are bundler-level and already handled in the unpacker, not here.

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
matching; it is handled by the esbuild-specific stateful matcher in
`un_template_literal.rs` rather than the central body-shape scanner.

## Handling version drift

Babel helpers do change across versions (bug fixes, spec compliance, browser capability changes). The solution is **relaxed matching** — check the essential semantic structure, not exact AST equality.

For `interopRequireDefault`, the essential structure has been stable for years because it's defined by the ES module spec: "if `__esModule`, return as-is; otherwise wrap in `{default: ...}`". If a future version fundamentally changes what a helper *does*, it's a new helper and gets a new matcher.

In practice, most variation across versions is:
- Different conditional forms (ternary vs if/else)
- Property access style (`.default` vs `["default"]`)
- Extra `Object.defineProperty` for non-configurable exports
- Added null checks

A good matcher checks for the presence of `__esModule` and `default` in the right structural positions, and tolerates everything else.
