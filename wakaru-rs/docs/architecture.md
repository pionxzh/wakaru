# Architecture

## What wakaru-rs does

Takes minified or bundled JavaScript and produces readable, modern JavaScript.

Two main operations:
1. **Decompile** — apply transformation rules to a single JS file
2. **Unpack + decompile** — split a bundle into modules, then decompile each

## High-level flow

```
                        ┌─────────────┐
                        │  input.js   │
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │   Unpacker  │  detects bundle format,
                        │  (optional) │  extracts module code
                        └──────┬──────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
         module_0.js      module_1.js      entry.js
              │                │                │
              ▼                ▼                ▼
        ┌───────────────────────────────────────────┐
        │              Decompile pipeline            │
        │                                            │
        │  parse → resolver → rules → fixer → emit  │
        │                                            │
        │  (parallel via rayon when unpacking)        │
        └───────────────────────────────────────────┘
              │                │                │
              ▼                ▼                ▼
         readable JS      readable JS      readable JS
```

## Components

### Unpackers (`src/unpacker/`)

Each unpacker detects a specific bundle format and extracts individual modules as raw JS strings. Detection is attempted in order — first match wins:

1. **webpack5** — IIFE/arrow with module factory array or object
2. **webpack4** — `(function(modules) { ... })([...])` with `__webpack_require__` runtime
3. **browserify** — `(function e(t,n,r) { ... })({1:[function(...){...}, {...}], ...})`
4. **esbuild** — scope-hoisted ESM with lazy-module helpers (`__commonJS` / `__esm`)

Unpackers emit raw module code. They do NOT run transformation rules — that's the driver's job. Webpack4 is the exception: it applies webpack-specific normalization (param rename, `require()` rewriting, runtime helper removal) before emitting, because those transforms are tightly coupled to the webpack format.

### Driver (`src/driver.rs`)

Orchestrates the full pipeline.

**`decompile(source, options)`** — single-file decompilation:
```
parse_js(source)
  → resolver(unresolved_mark, top_level_mark)
  → apply_default_rules(module, unresolved_mark)
  → [optional: source map rename pipeline]
  → fixer()
  → print_js(module)
```

**`unpack(source, options)`** — bundle splitting + parallel decompilation:
```
unpack_bundle(source)
  → rayon::par_iter over modules
  → decompile(module.code, options) for each
  → collect results
```

### Rules pipeline (`src/rules/`)

~45 transformation rules, each implementing SWC's `VisitMut` trait. Applied in a fixed order by `apply_default_rules()`. Order matters — some rules depend on earlier ones having run.

#### Pipeline stages

```
Stage 1: Syntax normalization
  SimplifySequence, FlipComparisons, RemoveVoid, UnminifyBooleans,
  UnInfinity, UnIndirectCall, UnTypeof, UnNumericLiteral, UnBracketNotation

Stage 2: Transpiler helper unwrapping
  UnInteropRequireDefault, UnInteropRequireWildcard, UnToConsumableArray,
  UnObjectSpread, UnSlicedToArray

Stage 3: Structural restoration
  UnTemplateLiteral, UnUseStrict, UnWhileLoop, UnCurlyBraces,
  UnTypeConstructor, UnEsmoduleFlag, UnAssignmentMerging, UnBuiltinPrototype,
  UnArgumentSpread, ObjectAssignSpread, UnVariableMerging,
  UnNullishCoalescing, UnOptionalChaining

Stage 4: Bundler artifacts
  UnWebpackInterop, UnIife, UnConditionals, UnParameters, UnEnum

Stage 5: Complex pattern restoration
  UnJsx, UnEs6Class, UnAsyncAwait, UnWebpackInterop (2nd pass), UnEsm

Stage 6: Modernization
  VarDeclToLetConst, ObjShorthand, ObjMethodShorthand, Exponent,
  ArgRest, UnRestArrayCopy, ArrowFunction, ArrowReturn

Stage 7: Cleanup and renaming
  UnWebpackDefineGetters, UnWebpackObjectGetters, UnImportRename,
  UnExportRename, SmartInline, UnIife (2nd pass), SmartRename, UnReturn
```

#### Key design pattern: `unresolved_mark`

After `resolver()` runs, every identifier gets a `SyntaxContext`. Free variables (globals like `Object`, `JSON`, `require`) are marked with `unresolved_mark`. This is how rules distinguish between:
- A local variable named `e` (has a bound SyntaxContext)
- The global `Object` (has `unresolved_mark` as outer mark)

Rules that match identifiers by name **must** check `SyntaxContext` to avoid renaming/transforming the wrong binding:

```rust
// Guard: only match free-variable references, skip bound inner-scope identifiers
if id.ctxt.outer() != self.unresolved_mark {
    return;
}
```

Without this guard, a rule matching `e` (a webpack param name) would also rename `e` inside `function inner(e) { ... }` — a completely unrelated binding.

**Pattern to follow when adding new visitors:** always take `unresolved_mark: Mark` and gate identifier matches on `id.ctxt.outer() == self.unresolved_mark`.

> **Why not use SWC's built-in `rename()`?**
> `swc_ecma_transforms_base::rename::rename(map: &FxHashMap<Id, Atom>)` exists and is
> battle-tested, but requires pre-building a map of `(Atom, SyntaxContext)` keys — which
> is the same information our `unresolved_mark` guard checks. For the narrow
> webpack factory-param use case our approach is simpler and equally correct.
> If a more general rename feature is ever needed, migrate to `rename_with_config()`.

### Source map pipeline (`src/sourcemap_rename.rs`)

Optional enhancement when `--sourcemap` is provided. Runs **after** the rules pipeline for two reasons:
1. Rules detect patterns by minified names (`require`, `__generator`, `__esModule`). Renaming first would break pattern detection.
2. `ImportDedup` needs `UnEsm` to run first (converting `require()` → `import`), and must merge duplicates before rename so we rename one binding instead of five.

```
ImportDedup           → merge repeated imports from same source
apply_sourcemap_renames → recover original names via position lookup
UnImportRename        → clean up import aliases
```

Name recovery works by:
1. For each identifier at generated position `(line, col)`
2. Look up original position via source map mappings
3. Read the identifier at that position from `sourcesContent`
4. Vote on the best original name per binding (majority wins)

This works even when the `names` array is empty (common in esbuild output).

## Rule safety model

Rules are safe by default — they preserve program semantics. Some transformations are inherently lossy (the original toolchain discarded information). These can be offered as aggressive/unsafe options controlled by a flag:

```rust
// Pipeline receives the preference
apply_default_rules(module, unresolved_mark, aggressive: bool)

// Individual rules check it
module.visit_mut_with(&mut UnInteropRequireDefault { aggressive });

// Safe-only rules ignore it
module.visit_mut_with(&mut FlipComparisons);
```

Examples of safe vs aggressive:
- **Safe**: `!0` → `true` (lossless)
- **Safe**: `_interopRequireDefault(require("x"))` → `require("x")` (known pattern)
- **Aggressive**: `_interopRequireWildcard(factory())` → `factory()` (drops namespace synthesis)
- **Aggressive**: `_extends(target, source)` → `{...target, ...source}` (drops mutation semantics)

## File structure

```
src/
  lib.rs              — public API
  main.rs             — CLI (clap)
  driver.rs           — decompile() and unpack() orchestration
  sourcemap_rename.rs — source-map-driven name recovery
  rules/
    mod.rs            — apply_default_rules() pipeline ordering
    babel_helper_utils.rs — shared helper detection (body shape + import path)
    rename_utils.rs   — shared binding rename utilities
    *.rs              — one file per transformation rule
  unpacker/
    mod.rs            — unpack_bundle() dispatch
    webpack4.rs       — webpack4 splitter + normalization
    webpack5.rs       — webpack5 splitter
    browserify.rs     — browserify splitter
    esbuild.rs        — esbuild splitter
  utils/
    matcher.rs        — AST helper predicates

tests/
  common/mod.rs       — render(), normalize(), assert_eq_normalized()
  *_rule.rs           — per-rule unit tests
  webpack4_unpack.rs  — pipeline snapshot tests (post-rules)
  webpack4_unpack_raw.rs — pipeline snapshot tests (pre-rules)
  esbuild_unpack.rs   — esbuild detection tests
  bundle_unpack.rs    — webpack5 + browserify tests
  noop_pipeline.rs    — stability tests
  snapshots/          — insta snapshot files

docs/
  architecture.md     — this file
  helper-detection.md — transpiler helper detection design
```

## References

- [SWC Architecture](https://github.com/swc-project/swc/blob/main/ARCHITECTURE.md)
- [SWC Rustdoc](https://rustdoc.swc.rs/swc/)
