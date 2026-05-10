# Architecture

## What wakaru does

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
4. **esbuild / Bun-compatible scope-hoisted ESM** — scope-hoisted ESM
   namespace boundaries (`__export(ns, ...)`) and esbuild lazy-module helpers
   (`__commonJS` / `__esm`)

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

**`unpack(source, options)`** — bundle splitting + two-phase parallel decompilation
(see "Multi-module pipeline" section below for the full two-phase design):
```
unpack_bundle(source)
  → Phase 1: par_iter → Stage 1+2 → collect facts → discard AST
  → Phase 2: par_iter → Stage 1+2 → late pass → Stage 3+ → emit
```

**`unpack_raw(source)`** — bundle splitting without the decompiler rule pipeline.
Returns raw module code as produced by the unpacker.

**`trace_rules(source, options, trace_options)`** — single-file rule tracing.
Runs the pipeline with an observer that captures per-rule before/after snapshots.

**`format_trace_events(events)`** — renders trace events as git-style unified diffs.

### Rules pipeline (`src/rules/`)

~60 transformation rules, each implementing SWC's `VisitMut` trait. Applied in a fixed order by `apply_default_rules()`. Order matters — some rules depend on earlier ones having run.

#### Pipeline stages

```
Stage 1: Syntax normalization
  SimplifySequence, FlipComparisons, UnTypeofStrict, RemoveVoid,
  UnminifyBooleans, UnDoubleNegation, UnInfinity, UnIndirectCall,
  UnTypeof, UnNumericLiteral, UnBracketNotation

Stage 2: Transpiler helper unwrapping + module-system reconstruction
  UnInteropRequireDefault, UnInteropRequireWildcard, UnToConsumableArray,
  UnObjectSpread, UnObjectRest, UnSlicedToArray, UnDefineProperty,
  UnClassCallCheck, UnPossibleConstructorReturn, UnTypeofPolyfill,
  UnCurlyBraces, UnEsmoduleFlag, UnUseStrict, UnAssignmentMerging,
  UnWebpackInterop, UnEsm

  ── cross-module barrier (unpack only: fact collection + late pass) ──

Stage 3: Structural restoration
  UnTemplateLiteral, UnWhileLoop, UnTypeConstructor, UnBuiltinPrototype,
  UnArgumentSpread, UnArrayConcatSpread, UnSpreadArrayLiteral,
  ObjectAssignSpread, UnVariableMerging, UnNullishCoalescing,
  UnOptionalChaining

Stage 4: Complex pattern restoration
  UnIife, UnConditionals, UnParameters, UnEnum, UnJsx, UnEs6Class,
  UnClassFields, UnTsHelpers, UnAsyncAwait, UnWebpackInterop (2nd pass)

Stage 5: Modernization
  UnThenCatch, UnUndefinedInit, VarDeclToLetConst, ObjShorthand,
  ObjMethodShorthand, UnPrototypeClass, Exponent, ArgRest,
  UnRestArrayCopy, ArrowFunction, ArrowReturn, UnForOf

Stage 6: Cleanup and renaming
  UnWebpackDefineGetters, UnWebpackObjectGetters, UnImportRename,
  UnExportRename, UnDestructuring, UnParameters (2nd pass),
  SmartInline, UnIife (2nd pass), SmartRename,
  [optional] DeadImports, [optional] DeadDecls, UnReturn
```

`DeadImports` and `DeadDecls` are an optional late cleanup phase controlled by
`DecompileOptions.dead_code_elimination`. They stay enabled for normal
decompilation output, but tests can disable them to snapshot structural
restoration separately from dead-code cleanup.

`DecompileOptions.level` is a separate user-facing control over rewrite
aggressiveness:

- `minimal` — prefer direct, local, high-confidence rewrites. Avoid recovery that
  depends on assuming compiler/transpiler output, synthesizing new bindings for
  readability, or reconstructing params from `arguments`.
- `standard` — the default decompiler mode. Recover common generated-source
  patterns when the evidence is strong and local, even if the rewrite is not a
  perfect edge-case semantics match. This is the readability-oriented mode wakaru
  is primarily tuned for.
- `aggressive` — enable speculative or compiler-intent-heavy recovery. This is
  where temp-erasing, alias-synthesizing, or fact-backed heuristics belong when
  they produce better recovered source but the proof is weaker.

These levels are a rewrite policy, not a formal semantics guarantee. In
particular, `standard` intentionally includes established decompiler heuristics
that favor likely original source over strict preservation of every JavaScript
edge case.

Rules are expected to gate risky subpatterns inside the rule, not by moving entire
rules in or out of the pipeline. This keeps pipeline ordering stable while allowing
specific rewrites to opt into `standard` or `aggressive` behavior.

Useful mental model:

| Level | Provenance assumption | Semantic risk | Structural synthesis |
|-------|-----------------------|---------------|----------------------|
| `minimal` | Low | Low | Low |
| `standard` | Medium | Moderate | Moderate |
| `aggressive` | High | Higher | High |

Examples from the current rollout:

- `minimal`
  - keeps plain strict optional-chaining recovery
  - skips `.apply(...)` → spread recovery
  - skips `arguments[...]`-based parameter reconstruction
  - skips object-alias default-param recovery
  - skips IIFE param renames / literal hoists
- `standard`
  - enables loose optional chaining on plain bases
  - enables `.apply(undefined, args)` / `obj.fn.apply(obj, args)` spread recovery
  - enables IIFE param cleanup and literal hoisting
  - enables object-alias and `arguments[...]`-based parameter recovery
- `aggressive`
  - enables optional-chaining assignment-temp recovery
  - enables JSX dynamic-tag alias synthesis
  - is reserved for future fact-backed cross-module heuristics

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

## Multi-module pipeline (`driver.rs`)

When unpacking bundles, the driver runs a two-phase pipeline:

1. **Phase 1 (parallel):** Parse each module → run Stage 1+2 → extract import/export facts → discard AST
2. **Phase 2 (parallel):** Parse each module again → run Stage 1+2 → cross-module late pass (re-export consolidation, namespace decomposition) → run Stage 3+ → emit

The late pass uses facts from Phase 1 to inform cross-module rewrites (e.g., converting `ns.foo` to `import { foo }`). Facts are extracted in `facts.rs` and consumed by `namespace_decomposition.rs` and `reexport_consolidation.rs`.

Stage 1+2 runs twice per module — once for fact collection, once for the real pipeline. This is necessary because SWC's `SyntaxContext` must remain continuous across the entire pipeline (re-parsing creates fresh contexts that break rename rules).

## File structure

```
src/
  lib.rs                      — public API exports
  main.rs                     — CLI entry point (clap)
  driver.rs                   — decompile() and unpack() orchestration
  facts.rs                    — post-Stage-2 cross-module fact extraction
  sourcemap_rename.rs         — source-map-driven name recovery
  namespace_decomposition.rs  — cross-module namespace-to-named-import rewrite
  reexport_consolidation.rs   — cross-module re-export consolidation
  rules/
    mod.rs                    — apply_default_rules() pipeline ordering
    babel_helper_utils.rs     — shared helper detection (body shape + import path)
    rename_utils.rs           — shared binding rename utilities
    *.rs                      — one file per transformation rule
  unpacker/
    mod.rs                    — unpack_bundle() dispatch
    webpack4.rs               — webpack4 splitter + normalization
    webpack5.rs               — webpack5 splitter
    browserify.rs             — browserify splitter
    esbuild.rs                — esbuild splitter
  utils/
    matcher.rs                — AST helper predicates

tests/
  common/mod.rs               — test helpers (see docs/testing.md)
  *_rule.rs                   — per-rule unit tests
  webpack4_unpack.rs          — pipeline snapshot tests (post-rules)
  webpack4_unpack_raw.rs      — pipeline snapshot tests (pre-rules)
  esbuild_unpack.rs           — esbuild detection tests
  bundle_unpack.rs            — webpack5 + browserify tests
  noop_pipeline.rs            — stability tests
  snapshots/                  — insta snapshot files

docs/
  architecture.md             — this file
  helper-detection.md         — transpiler helper detection design
  debugging.md                — rule tracing, snapshot debugging, fixture workflow
  testing.md                  — test patterns, helpers, organization
```

## References

- [SWC Architecture](https://github.com/swc-project/swc/blob/main/ARCHITECTURE.md)
- [SWC Rustdoc](https://rustdoc.swc.rs/swc/)
