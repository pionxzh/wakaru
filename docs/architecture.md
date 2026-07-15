# Architecture

## What wakaru does

Takes minified or bundled JavaScript and produces readable, modern JavaScript.

Two main operations:
1. **Decompile** — apply transformation rules to a single JS file
2. **Unpack + decompile** — split one or more bundle/chunk inputs into modules,
   then decompile each

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

### Unpackers (`crates/core/src/unpacker/`)

Each unpacker detects a specific bundle format and extracts individual modules as raw JS strings. Detection is attempted in order — first match wins:

1. **webpack5** — IIFE/arrow with module factory array or object, including
   runtime-only entry files and Vercel ncc's inline startup variant
2. **webpack4** — `(function(modules) { ... })([...])` with `__webpack_require__` runtime
3. **webpack5 chunk** — JSONP chunk push with a webpack module object
4. **browserify** — `(function e(t,n,r) { ... })({1:[function(...){...}, {...}], ...})`
5. **SystemJS** — top-level `System.register(...)` modules
6. **esbuild / Bun** — scope-hoisted ESM namespace boundaries
   (`__export(ns, ...)`) and CJS factory helpers (`__commonJS` / `__esm`).
   Bun's bundler emits the same helper shapes as esbuild, so CJS-interop
   bundles from Bun are detected and split by this unpacker.
   Preserved Bun path comments are used only as filename hints for modules
   already found through structural helper patterns; they are not module
   boundaries by themselves.

If nothing matches directly, `wrappers.rs` unwraps UMD factory and AMD
`define()` wrapper shapes and retries the same detection chain on each
unwrapped candidate. Finally, **AMD** (`amd.rs`) detects files consisting of
top-level `define(id, deps, factory)` calls and splits each define into a
module.

Vercel ncc CommonJS output with an IIFE webpack bootstrap is handled as a
webpack5 producer, not as a separate bundle format. Its module table is
extracted normally, while the statements beginning at the binding ultimately
assigned to `module.exports` become a synthetic `entry.js`.
`__nccwpck_require__` calls are normalized to `require()` and numeric module
IDs are rewritten to the emitted module filenames. This recovers the
JavaScript module graph; files emitted separately by ncc's asset relocation
loader are not reconstructed by the unpacker. ncc's `.mjs` output uses a
top-level runtime rather than this IIFE shape and is not structurally split.

Pure ESM scope-hoisted output (from esbuild, Bun, Rollup, or Vite) without
`__export` / `__commonJS` markers has no runtime markers to detect. When no
bundle format matches, the driver falls back to heuristic scope-hoisted
splitting (`scope_hoist.rs`, format `scope-hoisted`): it clusters top-level
declarations by reference graph and emits one module per cluster. This
fallback is on by default for `--unpack` (disabled by `--unpack=strict`) and
requires a minimum declaration count plus at least two clusters; otherwise
the file goes through single-file decompile. The same splitter also runs on
detected modules to break up scope-hoisted chunks nested inside another
bundle format.

Unpackers emit module code strings. They do not run the normal decompile rule
pipeline — that's the driver's job. Bundler-specific extraction normalization
(factory parameter renaming, `require()` rewriting, and runtime helper
removal) remains in the relevant unpacker because those transforms are tightly
coupled to the bundle format.

### Driver (`crates/core/src/driver.rs`, `crates/core/src/driver/`)

Orchestrates the full pipeline.

**`decompile(source, options)`** — single-file decompilation:
```
parse_js(source)
  → resolver(unresolved_mark, top_level_mark)
  → apply_rules(module, unresolved_mark, RulePipelineOptions)
  → [optional: source map rename pipeline]
  → fixer()
  → print_js(module)
```

**`unpack(source, options)`** — single-source bundle splitting + two-phase
parallel module decompilation (see "Multi-module pipeline" section below for
the full design):
```
unpack_bundle(source)
  → detector payload: normalized source or prepared AST
  → Phase 1: par_iter → obtain resolved AST → rules through UnEsm
                        → ESM recovery on a facts clone → collect facts
  → Phase 2: par_iter → resume retained AST → cross-module late pass
                    → rules from UnTemplateLiteral through UnReturn
                    → targeted late cleanup → emit
```

**`unpack_files(inputs, options)`** — multi-source unpack for an entry plus
chunk files. Each input is detected independently, detected module sets are
merged, and the same two-phase pipeline runs once over the combined module set
so cross-module facts can see modules from every input file.

Before the two-phase pipeline starts, multi-source unpack stabilizes the merged
module set: filenames are made unique before fact collection, and unambiguous
numeric webpack module IDs are mapped to those final filenames so entry/chunk
references can be rewritten across physical input files. Duplicate numeric IDs
are treated as ambiguous and are not rewritten globally, which avoids merging
unrelated webpack runtimes from the same scanned directory.

**`unpack_raw(source)`** — bundle splitting without the normal decompile rule
pipeline. It returns detector output after only the extraction and
bundler-coupled cleanup needed to make each extracted module stand alone.
Webpack/browserify extractors use named extraction normalization helpers for
that boundary work, such as factory parameter renaming, numeric/string module
ID rewrites, `require.n` access normalization, and wrapper/decorator removal.
They do not run a slice of the normal rule pipeline. Webpack ESM markers and
export getters remain in raw output so the later decompile pipeline can recover
live ESM exports without guessing.

**`unpack_files_raw(inputs)`** — multi-source raw unpack. It merges raw
detector output from all inputs and skips the normal decompile pipeline.

The CLI also accepts directory inputs with `--unpack`. Directory inputs are
expanded recursively to `.js`, `.mjs`, and `.cjs` candidates while skipping
hidden files/directories and `node_modules`. Directory-discovered files are
detect-only: files that do not match a bundle/chunk shape are skipped rather
than copied or decompiled. Explicit file inputs keep the normal single-file
fallback behavior.

**`trace_rules(source, options, trace_options)`** — single-file rule tracing.
Runs the pipeline with an observer that captures per-rule before/after snapshots.

**`format_trace_events(events)`** — renders trace events as git-style unified diffs.

### Rules pipeline (`crates/core/src/rules/`)

~60 transformation rules, each implementing SWC's `VisitMut` trait. Applied in a fixed order by `apply_rules()`. Order matters — some rules depend on earlier ones having run. The ordered registry lives in `crates/core/src/rules/pipeline.rs` as `RuleDescriptor` entries with `RuleStage` metadata and explicit ordering dependencies, while `RulePipelineOptions` controls ranges, rewrite level, dead-code cleanup, and optional module facts.

#### Pipeline stages

```
Stage 1: Syntax normalization
  SimplifySequence, FlipComparisons, UnTypeofStrict, RemoveVoid,
  UnminifyBooleans, UnDoubleNegation, UnInfinity, UnIndirectCall,
  UnTypeof, UnNumericLiteral, UnBracketNotation

Stage 2: Transpiler helper unwrapping + module-system reconstruction
  UnInteropRequireDefault, UnInteropRequireWildcard, UnToConsumableArray,
  UnObjectSpread, UnObjectRest, UnSlicedToArray,
  UnClassCallCheck, UnPossibleConstructorReturn,
  UnTypeofPolyfill, UnCurlyBraces, UnEsmoduleFlag, UnUseStrict,
  UnAssignmentMerging, UnVariableMergingDeclsOnly, UnBuiltinAliases,
  UnWebpackInterop, UnEsm

  ── cross-module barrier (unpack only: fact collection + late pass) ──

Stage 3: Structural restoration
  UnTemplateLiteral, UnWhileLoop, UnTypeConstructor, UnBuiltinPrototype,
  UnArgumentSpread, UnArrayConcatSpread, UnSpreadArrayLiteral,
  ObjectAssignSpread, UnVariableMerging, UnNullishCoalescing,
  UnOptionalChaining

Stage 4: Complex pattern restoration
  UnIife, UnConditionals, UnParameters, UnEnum, UnJsx, UnEs6Class,
  UnAssertThisInitialized, UnClassFields, UnDefineProperty,
  UnRegenerator, UnAsyncAwait, UnWebpackInterop (2nd pass)

Stage 5: Modernization
  UnThenCatch, UnUndefinedInit, VarDeclToLetConst, ObjShorthand,
  ObjMethodShorthand, UnPrototypeClass, Exponent, ArgRest,
  UnRestArrayCopy, ArrowFunction, UnNamespace, ArrowReturn, UnForOf

Stage 6: Cleanup and renaming
  UnWebpackDefineGetters, UnWebpackObjectGetters, ImportDedup,
  UnImportRename, UnExportRename, UnWebpackInterop (3rd pass),
  UnDestructuring, UnParameters (2nd pass), SmartInline,
  UnIife (2nd pass), SmartRename, UnJsx (2nd pass),
  [optional] DeadDecls, [optional] DeadImports, UnReturn
```

`DeadImports` and `DeadDecls` are an optional late cleanup phase controlled by
`DecompileOptions.dce_mode`. CLI output uses transform-only cleanup by default,
preserving dead code that was already dead in the input while removing
transform-induced leftovers. Transform-only cleanup also retains original ESM
import specifiers: even an otherwise-unused default or named import performs an
observable link-time export check. It removes only dead import specifiers that
the rewrite pipeline synthesized. `--dce` opts into a full reachability sweep.
Tests and API callers can set `DceMode::Off` to snapshot structural restoration
separately from dead-code cleanup.

Unpacked bundle modules are the complementary case: their ESM imports are
Wakaru's recovered representation of bundle edges, not source-level link
checks. Transform-only cleanup may therefore remove a recovered specifier when
a later rewrite removes its last use, while retaining the side-effect import.
The unpack driver snapshots specifiers that were already dead at the phase-2
barrier and runs a final recovered-import cleanup after the targeted late
rewrites, so only specifiers made dead by those rewrites are removed.

`DecompileOptions.level` controls rewrite aggressiveness — `minimal` (high
confidence, semantics-preserving), `standard` (default, readability-oriented),
or `aggressive` (speculative recovery). Rules gate risky subpatterns inside the
rule rather than moving entire rules in or out of the pipeline.

See [Rewrite assumptions](rewrite-assumptions.md) for the semantic contract:
which named assumptions each level may rely on, and the reproduce-first policy
for new heuristics.

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

**Known deviation: Vue SFC recovery (being retired).** The experimental
`--vue-sfc` recovery path (`crates/core/src/vue_recovery.rs` and
`crates/core/src/vue_recovery/`) re-parses printed JavaScript and runs
`resolver()` over it. Identifier matching is now `SyntaxContext`-gated like the
main pipeline: helper recognition, alias/props renaming (via
`rename_utils::BindingRenamer`), and the reference collectors all key on
`(name, ctxt)`; the hand-rolled `ScopeStack` is gone. What remains as
implementation debt: the IR (`VueExpr`) still carries printed *strings*, so
template-expression reference collection and prefix renaming go through
string-level lexers (`vue_recovery/js_refs.rs`, `rename_code_segment`) rather than
the AST. Removing that string machinery by carrying the resolved AST in the IR is
the last step of the resolver redesign (issue #196; see the sequencing plan).
Treat the remaining string passes as debt of the experimental Vue subsystem, not
a precedent for new rules in the main decompile pipeline.

> **Why not use SWC's built-in `rename()`?**
> `swc_ecma_transforms_base::rename::rename(map: &FxHashMap<Id, Atom>)` exists and is
> battle-tested, but requires pre-building a map of `(Atom, SyntaxContext)` keys — which
> is the same information our `unresolved_mark` guard checks. For the narrow
> webpack factory-param use case our approach is simpler and equally correct.
> If a more general rename feature is ever needed, migrate to `rename_with_config()`.

### Source map pipeline (`crates/core/src/sourcemap_rename.rs`)

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

## Multi-module pipeline (`crates/core/src/driver/unpack.rs`)

When unpacking bundles, the driver runs a two-phase pipeline:

1. **Phase 1 (parallel):** Obtain a resolved module AST. Source-only detector
   output is parsed and resolved here; webpack5 can hand off its already
   resolved, bundler-normalized AST directly. Run the rule registry through
   `UnEsm`, clone that barrier AST for webpack factory-IIFE fact recovery, and
   extract import/export facts. Retain the pre-recovery AST together with its
   `Globals` and unresolved mark.
2. **Phase 2 (parallel):** Resume the retained Phase 1 AST → cross-module late
   pass (re-export consolidation, namespace decomposition, fact-aware helper
   recovery) → run the `UnTemplateLiteral` through `UnReturn` rule range →
   targeted late cleanup/recovery → emit.

The late pass uses facts from Phase 1 to inform cross-module rewrites (e.g., converting `ns.foo` to `import { foo }` or recognizing a split helper module). Facts are extracted in `crates/core/src/facts.rs` and consumed by `crates/core/src/namespace_decomposition.rs`, `crates/core/src/reexport_consolidation.rs`, and fact-aware rules. See [fact-system.md](fact-system.md) for details.

Normal no-source-map unpack runs the through-`UnEsm` range once and carries the
same `Globals`/`SyntaxContext` lineage across the barrier. If Phase 1 cannot
prepare an AST, Phase 2 retains the best-effort parser fallback. Output
source-map mode also deliberately materializes prepared detector ASTs and uses
the parser path because its mappings depend on parser-owned per-module source
coordinates.

The internal detector handoff is a single aligned payload boundary rather than
a format branch in either phase: each module has source text and may also have a
private prepared AST sidecar. Public/raw unpack APIs materialize every sidecar
back into `UnpackedModule::code`; the normal driver consumes it once at the
Phase 1 boundary. Aggressive nested scope splitting likewise materializes first
because that pass operates on emitted module text.

## File structure

```
crates/
  cli/
    src/
      main.rs                       — CLI entry point (clap)

  core/
    src/
      lib.rs                        — public API exports
      driver.rs                     — public driver facade
      driver/
        single_file.rs              — decompile() orchestration
        unpack.rs                   — unpack(), unpack_raw(), and multi-module pipeline
        trace.rs                    — rule trace orchestration and formatting
        diagnostics.rs              — post-transform diagnostic warning collection
        discovery.rs                — recursive input-directory scan + bundle detection
        output.rs                   — output-path safety, dedup, write-if-changed
        io.rs                       — parse/print helpers
        types.rs                    — driver options, outputs, and warning types
      facts.rs                      — post-Stage-2 cross-module fact extraction
      sourcemap_rename.rs           — source-map-driven name recovery
      namespace_decomposition.rs    — cross-module namespace-to-named-import rewrite
      reexport_consolidation.rs     — cross-module re-export consolidation
      rules/
        mod.rs                      — rule module declarations and public exports
        pipeline.rs                 — rule descriptor registry and pipeline execution
        transpiler_helper_utils/    — shared helper detection (module dir)
          mod.rs                    — helper-kind types, LocalHelperContext, shared AST predicates
          collect.rs                — module-level helper scan/orchestration
          matchers.rs               — Babel/SWC body-shape matchers + per-node detection dispatch
          ts_helpers.rs             — TypeScript/tslib detection (raw TsHelperKind channel)
          paths.rs                  — runtime import-path constants + path classification
          lifecycle.rs              — helper-declaration reference tracking + removal
        match_context.rs            — binding-aware slots for helper body matchers
        helper_matcher.rs           — shared helper binding/lifecycle primitives
        rename_utils.rs             — shared binding rename utilities
        *.rs                        — one file per transformation rule
      unpacker/
        mod.rs                      — unpack_bundle() dispatch
        webpack4.rs                 — webpack4 splitter + normalization
        webpack5.rs                 — webpack5 splitter (incl. runtime entry, ncc + chunk)
        browserify.rs               — browserify splitter
        systemjs.rs                 — System.register splitter + ESM reconstruction
        esbuild.rs                  — esbuild/Bun splitter (CJS factories + scope-hoisted)
        amd.rs                      — AMD define() bundle splitter
        wrappers.rs                 — UMD/AMD wrapper unwrapping for detection retry
        scope_hoist.rs              — heuristic scope-hoisted splitting (esbuild, Bun, Rollup, Vite)
      utils/
        matcher.rs                  — AST helper predicates
    tests/
      common/mod.rs                 — test helpers (see docs/testing.md)
      *_rule.rs                     — per-rule unit tests
      *_unpack.rs                   — per-bundler unpack/pipeline snapshot tests
                                      (webpack4 + raw, webpack5 chunk, bundle_unpack
                                      = webpack5 + browserify, esbuild, systemjs,
                                      amd, rollup, bun, multi-file)
      webpack_fixtures.rs           — generated webpack4/5 + ncc fixture coverage
      noop_pipeline.rs              — stability tests
      snapshots/                    — insta snapshot files

  wasm/
    src/
      lib.rs                        — wasm-bindgen entry point (decompile + unpack)

docs/
  architecture.md                   — this file
  testing.md                        — test patterns, helpers, organization
  debugging.md                      — rule tracing, snapshot debugging, fixture workflow
  helper-detection.md               — transpiler helper detection design
  fact-system.md                    — cross-module fact system
  rule-dependency-inventory.md      — rule dependency relationships
  rewrite-assumptions.md            — semantic assumptions and rewrite policy
  releasing.md                      — changelog and release workflow
  test262-roundtrip.md              — semantic round-trip runner and baselines
  test262-baselines/                — tracked Test262 baseline summaries
  proposals/                        — design proposals (deferred or in progress)
  learnings/                        — approaches that were built, measured, and rejected
```

## Related docs

- [Testing](testing.md) -- test patterns, helpers, and organization
- [Debugging](debugging.md) -- rule tracing, snapshot debugging, fixture workflow
- [Helper detection](helper-detection.md) -- transpiler helper detection design
- [Fact system](fact-system.md) -- cross-module fact system
- [Rule dependency inventory](rule-dependency-inventory.md) -- rule dependency relationships and experimental validation
- [Rewrite assumptions](rewrite-assumptions.md) -- semantic assumptions and rewrite policy
- [Vue decompile](vue-decompile.md) -- no-sourcemap Vue render recovery and SFC printer scope

## References

- [SWC Architecture](https://github.com/swc-project/swc/blob/main/ARCHITECTURE.md)
- [SWC Rustdoc](https://rustdoc.swc.rs/swc/)
