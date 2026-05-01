# Helper Detection Design

Design notes for detecting and restoring transpiler runtime helpers in wakaru-rs.

## Problem

Transpilers (Babel, TypeScript/tslib, SWC) inject runtime helper functions to polyfill modern syntax for older targets. In bundled output, these helpers appear in several forms:

1. **Imported** from a runtime package — `require("@babel/runtime/helpers/interopRequireDefault")`
2. **Inlined** at the top of each module — the function body is copied directly, no import
3. **Hoisted** into a shared webpack module — accessed via numeric `require(42)`, name lost entirely
4. **Minified** — parameter and function names are mangled, but the body structure is preserved

The JS wakaru handles case 1 (match by import path). wakaru-rs should handle all four.

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

These ideas were explored (via Grok brainstorm + Codex review) and rejected:

- **Custom IR layer** — SWC's AST is already high-level (has CallExpr, AwaitExpr, YieldExpr, etc.). A second IR would duplicate representation and debugging cost without solving the actual problems. We already do generator-to-async restoration directly on SWC AST in `un_async_await.rs`.

- **CFG hashing / structural fingerprints** — Sounds appealing but fragile in practice. Small codegen/minifier changes scramble naive hashes, and stable canonicalization is the hard part (not storing the graph). Overkill for functions that are typically 1-5 lines.

- **Version auto-detect via runtime strings** — Bundled code often strips version markers, and inlined helpers erase them entirely. Designing around version gating would fail on real-world bundles.

- **Configurable pass graphs / incremental re-analysis** — Premature optimization. The current linear pipeline in `rules/mod.rs` is simple and works.

## Architecture

### Detection (`babel_helper_utils.rs`)

The `collect_helpers()` function scans module-level declarations (function declarations and function-assigned variables) and returns a `HashMap<BindingKey, BabelHelperKind>` by running each candidate through a set of shape matchers.

```
scan module-level declarations
  → for each function body, run shape matchers
  → collect (binding_key, BabelHelperKind) pairs
```

Shape matchers are plain functions: `fn(&Function) -> bool`. They check essential structural elements and ignore variable names. Writing a new matcher for a new helper is just writing a new predicate.

Helper functions include `collect_helpers_of_kind()` (filter by kind), `remove_helper_declarations()` (delete the helper function), and `helpers_with_remaining_refs()` (check if a helper binding is still referenced elsewhere).

### Restoration

Each helper kind has its own dedicated rule struct (e.g., `UnInteropRequireDefault`, `UnInteropRequireWildcard`, `UnClassCallCheck`). Each rule implements `VisitMut` and internally calls `collect_helpers()` to detect helpers, then rewrites call sites.

For example, `UnInteropRequireDefault`:
- `var _a = _interopRequireDefault(require("a"))` becomes `var _a = require("a")`
- `_a.default` becomes `_a` (at all reference sites)
- The helper function declaration is removed

### Where it runs in the pipeline

Helper detection and restoration runs within **Stage 2** of the pipeline in `apply_default_rules()`, after Stage 1 syntax normalization. Stage 1 rules like `UnIndirectCall` and `UnBracketNotation` must run first to normalize patterns like `(0, x.default)()` and `["default"]` before helper detection can match reliably.

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

## Handling version drift

Babel helpers do change across versions (bug fixes, spec compliance, browser capability changes). The solution is **relaxed matching** — check the essential semantic structure, not exact AST equality.

For `interopRequireDefault`, the essential structure has been stable for years because it's defined by the ES module spec: "if `__esModule`, return as-is; otherwise wrap in `{default: ...}`". If a future version fundamentally changes what a helper *does*, it's a new helper and gets a new matcher.

In practice, most variation across versions is:
- Different conditional forms (ternary vs if/else)
- Property access style (`.default` vs `["default"]`)
- Extra `Object.defineProperty` for non-configurable exports
- Added null checks

A good matcher checks for the presence of `__esModule` and `default` in the right structural positions, and tolerates everything else.
