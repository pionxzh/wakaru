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

### Bun single-file executable containers (`crates/wakaru/src/bun.rs`)

A Bun single-file executable is a native PE, Mach-O, or ELF host followed by a
serialized module graph. It is a container around compiled outputs, not itself
a JavaScript bundle format. The parser finds Bun's
`\n---- Bun! ----\n` trailer, reads the preceding offsets record, validates the
36-byte record layout used by Bun 1.3.3–1.3.8 or the 52-byte layout introduced
in Bun 1.3.9, validates every data pointer, and returns exact borrowed slices.

For explicit executable inputs, the normal `--unpack` path hands JS/JSX/TS/TSX
entries to one `UnpackJob`; the entry point is pushed first. Compiled Bun
entries use a parenthesized CommonJS container with the exact parameters
`exports`, `require`, `module`, `__filename`, and `__dirname`; the wrapper
intake exposes that body to the existing esbuild/Bun factory detector. This
second stage can recover thousands of inner factory and scope-hoisted modules.
The separate `bun extract` subcommand stops at the container boundary and
writes every file record byte-for-byte, including non-JavaScript assets.

Single-file executable entry bodies can be tens of megabytes, so this handoff moves the
wrapper body into the detector instead of cloning it, restoring the body if
detection rejects the candidate. Once an esbuild/Bun factory shape is accepted,
the detector moves unresolved factory bodies into their pending output modules
while keeping the resolved analysis AST separate; emission must not reuse the
resolved AST because that can change binding names and synthesized imports.
Resolved support-declaration dependencies are reduced to compact binding sets,
allowing the full analysis AST to be dropped before scope splitting and module
emission. Top-level source declarations are indexed by position and cloned only
when a recovered module needs to own them; the detector must not retain one AST
clone per binding.

This trailer-first parser is independent of the native executable format and
does not execute the input. The public `wakaru::bun` API borrows exact content
and metadata slices and reports their absolute executable byte ranges.
Opaque Bun source-map, bytecode, and module-info regions are available
separately; Bun's serialized source maps are not v3 JSON source maps. See
[bun-standalone.md](bun-standalone.md).

### Unpackers (`crates/core/src/unpacker/`)

Each unpacker detects a specific bundle format and extracts individual modules as raw JS strings. Detection is attempted in order — first match wins:

1. **webpack5** — IIFE/arrow with module factory array or object, including
   runtime-only entry files, inline startup (both webpack's own
   `var __webpack_exports__ = {}` form and Vercel ncc's variant), and the
   unwrapped `output.iife: false` form. `experiments.outputModule` bundles that
   carry top-level ESM `export`/`import` declarations are left untouched — their
   public surface can't yet be recovered faithfully
2. **webpack4** — `(function(modules) { ... })([...])` with `__webpack_require__` runtime
3. **webpack5 chunk** — JSONP chunk push with a webpack module object
4. **browserify family** — numeric-keyed
   `(function e(t,n,r) { ... })({1:[function(...){...}, {...}], ...})`,
   including Cocos Creator 2.x's string-keyed `window.__require` variant
5. **Closure ModuleManager** — Google/Closure shared-namespace module segments,
   usually guarded by loader `try/catch` blocks and optionally labeled by
   `/*_M:id*/`. Consecutive labels are retained as empty logical modules, and
   proven enclosing top-level and leading wrapper bootstrap code is preserved
   in each output. An unguarded statement in a direct response or after wrapper
   segment extraction begins rejects the shape rather than guessing placement.
   The `_ModuleManager_initialize(...)` graph is decoded to validate module
   identities and response ordering. Dependency indexes must refer to an
   earlier graph record, matching Closure Library's one-pass runtime decoder;
   forward indexes reject the candidate. Loader dependencies are not fabricated
   as ESM imports.
6. **SystemJS** — top-level `System.register(...)` modules
7. **esbuild / Bun** — scope-hoisted ESM namespace boundaries
   (`__export(ns, ...)`) and CJS factory helpers (`__commonJS` / `__esm`).
   Bun's bundler emits the same helper shapes as esbuild, so CJS-interop
   bundles from Bun are detected and split by this unpacker.
   Preserved Bun path comments are used only as filename hints for modules
   already found through structural helper patterns; they are not module
   boundaries by themselves.
8. **Metro** — React Native/Expo plain-JavaScript bundles made of top-level
   `__d(factory, moduleId, dependencyMap)` definitions and `__r(entryId)`
   startup calls. The extractor resolves indexed dependencies, normalizes the
   fixed seven factory parameters, and recovers Metro's default/namespace
   import loaders. When dynamic import, prefetch, maybe-sync, or `resolveWeak`
   leaves dependency-map accesses in the extracted module, the full map is
   preserved as a local binding. Every definition using the selected runtime
   prefix must parse before any modules are emitted, preventing partial tables
   with dangling imports. Indexed/file RAM bundles and Hermes bytecode are
   separate binary formats and are not handled by this detector.

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

Cocos Creator 2.x project-script bundles are treated as a Browserify dialect,
not as a new public `BundleFormat`. The detector recognizes the assignment to
`window.__require`, string module and entry IDs, `[factory, dependencyMap]`
tuples, and paired factory-scope `cc._RF.push/pop` registration markers. The
marker scan accepts top-level comma sequences produced by minifiers without
descending into nested functions. Dependency-map targets found in the same
table are rewritten to relative emitted filenames. When a request is absent
from the map, the extractor models Cocos's basename retry against named modules
in the same table; requests still unresolved after that remain intact because
Cocos can delegate them to a previously loaded `__require` bundle. Registration
markers are preserved because removing them would change Cocos runtime behavior.

Ordinary Browserify numeric module tables use an unambiguous dependency-map
request path as the emitted filename when every hint for that module agrees.
Ambiguous or missing hints retain `module-<id>.js`; entry names remain
`entry.js` / `entry-<id>.js`, and path collisions are suffixed
case-insensitively. Dependency rewrites always use the final emitted filename.

Factory-based webpack, Browserify/Cocos, and Metro extraction removes the
factory wrapper and gives its runtime parameters canonical names. Before doing
so, the unpackers check top-level collisions, pre-existing free references, and
nested-scope shadowing. Bound locals that would capture a canonical runtime
name are hygienically renamed first. A pre-existing free reference cannot be
renamed without changing host-environment lookup, so that case still rejects
the candidate and normal fallback preserves the original bundle.

Webpack has one narrower partial-failure path. A minified factory may reuse its
`module`, `exports`, or loader parameter as an ordinary local after its last
runtime use. In a numeric-ID container, when that lifetime boundary cannot be
proved, Wakaru preserves that factory's extracted body unchanged and marks only
that module as failed; other factories in the same structurally proven
container remain recoverable. Named-ID containers keep the whole-input fallback
because an unresolved path-like runtime call could otherwise be mistaken for an
ESM import. A fixed-point pass
removes failed factory IDs from the rewrite map before retrying dependants, so
calls to an opaque factory follow the existing absent-ID behavior instead of
becoming invented ESM edges. The opaque body never enters rule processing,
fact collection, filename recovery, or recursive scope splitting. Other
normalization failures still reject the whole container, and a container with
no recoverable factory still uses the original whole-input fallback.

For a provable reuse boundary, localization runs before webpack's ordinary,
position-insensitive runtime normalization. Only immediately evaluated uses
before the first unconditional write, plus supported loader uses in that
write's initializer, receive the canonical `module`, `exports`, or `require`
identity. The write is lifted to a new `var` local and every later use follows
that local. Runtime-helper members and mapped module calls in a
loader prefix can then use the normal webpack recovery path; post-write calls
and members stay attached to the new local value even when a numeric argument
happens to match a module-table ID. Webpack 5's top-level
`module = require.hmd(module)` / `nmd(module)` decorators are runtime-preserving
operations consumed by its existing normalizer, not lifetime boundaries. A
first real write may be a top-level assignment, a `var` redeclaration of the
factory parameter, or a direct element inside a top-level/initializer sequence
when splitting the sequence preserves its evaluation result. A consumed alias
reset on the guaranteed-once right-hand side of a top-level `for ... in` is
also supported by replacing the reset with the localized value before lifting
its initializer. Numeric calls
absent from the current table remain explicit `require(<number>)` runtime calls
and never synthesize an ESM edge. Webpack 5's pure `.g` and `.amdO`
runtime-member reads may occur in a conditional loader prefix because its
normalizer consumes them. Conditional first-write boundaries, hoisted or other
deferred pre-write captures, unmapped string IDs, consumed mid-sequence
assignment results, and `module` / `exports` initializers that read the old
runtime value remain failed/opaque rather than triggering control-flow or
facade inference.

Pure ESM scope-hoisted output (from esbuild, Bun, Rollup, or Vite) without
`__export` / `__commonJS` markers has no runtime markers to detect. When no
bundle format matches, the driver falls back to heuristic scope-hoisted
splitting (`scope_hoist.rs`, format `scope-hoisted`): it clusters top-level
declarations by reference graph and emits one module per cluster. This
fallback is on by default for `--unpack` (disabled by `--unpack=strict`) and
requires a minimum declaration count plus at least two clusters; otherwise
the file goes through single-file decompile. The same splitter also runs on
detected modules to break up scope-hoisted chunks nested inside another
bundle format. Synthetic clusters that form an import cycle are merged before
normal emission so the recovered ESM graph preserves the original single-file
initialization order. Internally, the splitter first builds a scope-hoist plan
containing the finest useful clusters and their reference graph, then selects
an emission policy. When one synthetic entry would otherwise turn a substantial
part of a large plan into a single cyclic component, executable rendering first
merges the underlying root SCCs and assigns singleton roots to contiguous
regions of a stable topological order; the final SCC merge still protects
initialization order. Small plans retain the established clustering behavior.
`--unpack=inspect` renders the original fine-grained plan recursively without
merging cyclic components; its finer module graph is for static inspection and
may not execute. Its cross-item write policy depends on where the source came
from. A direct scope-hoisted asset (a whole Rollup/Vite-style chunk) accepts a
write merge only when the writer's and owner's clusters are neighbors in
top-level item order (their item-index hulls overlap or touch): true modules
in such chunks are almost always one contiguous run of items, so a distant
runtime write hub is transitive glue, not same-module evidence. A nested
module body extracted from a structural bundle measured markedly weaker
contiguity, so there Inspect instead retains write merges when they connect at
most eight pre-existing clusters, and inside a larger write-connected
component also retains degree-one writer edges when their leaf-only residual
component stays within that cap — but only when doing so leaves the final
post-folding cluster count unchanged for each write component; both increases
and decreases fall back to the conservative component cap instead of being
allowed to cancel across independent components.

For corpus analysis, `cargo run -p wakaru-core --example scope_hoist_trace --
path/to/bundle.js` emits item ranges, Signal 1–5 clusters, cross-write topology,
and the selected Inspect partition as JSON. This is an internal research
surface rather than part of the supported `wakaru` façade API.

When Inspect splits one oversized write component into multiple fine modules,
each unambiguous child also carries the full pre-cap component ranges as
analysis context. Siblings therefore share one context identity without
sharing a package identity. A synthetic entry that folds items from several
components receives no context, and normal executable output always leaves the
field empty. Nested context is retained only when its generated ranges map
precisely back to the physical input.

Unpackers emit module metadata with source text and, when available, a private
prepared normalized AST sidecar. They do not run the normal decompile rule
pipeline — that's the driver's job. Prepared payloads cross the same Phase 1
boundary as source-only payloads; there is no format-specific rule route.
Bundler-specific extraction normalization (factory parameter renaming,
dependency-map or module-ID rewriting, and runtime helper removal) remains in
the relevant unpacker because those transforms are tightly coupled to the
bundle format. Detector-owned metadata can additionally carry a narrowly
proven runtime invariant into the normal driver when applying it during
extraction would violate raw passthrough. Webpack5 and Metro can hand their
normalized ASTs directly to Phase 1, avoiding an emit/parse cycle; raw unpack
and source-map mode materialize the sidecar to source text.
For numeric webpack factories, this metadata preserves the runtime type and
value of a syntactically numeric container key; the public string module ID
alone cannot distinguish `17` from `"17"`, and the latter does not prove the
runtime ID type. A separate legacy-container bit records when the
Webpack 4 `module.i` spelling is available. Exact CSS-loader runtime adapters
consume these facts during normal processing. Numeric identity substitution is
kept independent from any `module.exports` recovery in the same factory, while
the optional conditional-locals default requires a complete CommonJS runtime
surface proof. Recursively split children inherit neither detector fact.
Detector output may also carry a private per-module failure sidecar. The normal
driver turns it into an operational diagnostic plus
`ModuleStatus::DecompileFailed` while preserving the raw extracted body; raw
detector APIs may discard this metadata because raw output has no graph-quality
contract.

Development builds are a non-goal. Wakaru targets shipped, production
bundles; artifacts that only appear in dev-mode output — such as webpack's
`devtool: 'eval'` family, which wraps every module body in an `eval("...")`
string for fast rebuilds — are intentionally not recovered. Such bodies pass
through as-is rather than being unwrapped. Don't propose eval-string
unwrapping or other dev-build-only recovery work.

### Driver (`crates/core/src/driver.rs`, `crates/core/src/driver/`)

Orchestrates the internal pipeline. The stable Rust surface is the `wakaru`
façade crate. Cargo requires separately packaged dependencies to exist in the
registry, so `wakaru-core` is published in lockstep and exact-version pinned by
the façade, but its driver types are not a supported semver contract.

The façade exposes owned `Source` inputs, structured module artifacts and
diagnostics, and two root operations: `decompile` and `unpack`. `UnpackJob`
adds push-based intake for directory walkers. Each push performs detection
once; compatible plain JavaScript retains that resolved AST for Phase 1, and
prepared detector modules are neither emitted nor reparsed before rules.

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
                    → registry range resuming after UnEsm, through UnReturn
                    → targeted late cleanup → emit
```

**`unpack_files(inputs, options)`** — multi-source unpack for an entry plus
chunk files. Each input is detected independently, detected module sets are
merged, and the same two-phase pipeline runs once over the combined module set
so cross-module facts can see modules from every input file.

The legacy `wakaru-core` `unpack*` entry points exist only under the doc-hidden
`driver::test_support` namespace for the crate's integration tests. They adapt
the same `prepare_unpack_input` intake and structured executor result used by
the façade; no production caller or second detector loop remains.

Before the two-phase pipeline starts, multi-source unpack stabilizes the merged
module set: filenames are made unique before fact collection, and unambiguous
numeric webpack module IDs are mapped to those final filenames so entry/chunk
references can be rewritten across physical input files. Duplicate numeric IDs
are treated as ambiguous and are not rewritten globally, which avoids merging
unrelated webpack runtimes from the same scanned directory.

**`unpack_raw(source)`** — bundle splitting without the normal decompile rule
pipeline. It returns detector output after only the extraction and
bundler-coupled cleanup needed to make each extracted module stand alone.
Webpack/browserify/Metro extractors use named extraction normalization helpers
for that boundary work, such as factory parameter renaming, numeric/string
module ID rewrites, Metro dependency-map resolution, `require.n` access
normalization, and wrapper/decorator removal.
They do not run a slice of the normal rule pipeline. Webpack ESM markers and
export getters remain in raw output so the later decompile pipeline can recover
live ESM exports without guessing.

**`unpack_files_raw(inputs)`** — multi-source raw unpack. It merges raw
detector output from all inputs and skips the normal decompile pipeline.

The CLI also accepts directory inputs with `--unpack`. It expands directories
recursively to `.js`, `.mjs`, and `.cjs` candidates while skipping hidden
files/directories and `node_modules`, then pushes each candidate directly into
one `UnpackJob`. Plain directory candidates are skipped and released during
the walk; the CLI does not run a separate boolean detection preflight.

**`trace_rules(source, options, trace_options)`** — single-file rule tracing.
Runs the pipeline with an observer that captures per-rule before/after snapshots.

**`format_trace_events(events)`** — renders trace events as git-style unified diffs.

### Rules pipeline (`crates/core/src/rules/`)

~80 transformation rules, each implementing SWC's `VisitMut` trait, applied in a fixed order by `apply_rules()` across ~100 registry entries (several rules run repeat passes after later stages expose new instances of their pattern). Order matters — some rules depend on earlier ones having run. The ordered registry lives in `crates/core/src/rules/pipeline.rs` as `RuleDescriptor` entries with `RuleStage` metadata and explicit ordering dependencies, while `RulePipelineOptions` controls ranges, rewrite level, dead-code cleanup, and optional module facts.

#### Pipeline stages

**The registry is the authoritative roster** — this doc describes only what
each stage is for, with a few representative rules. Do not treat the examples
as exhaustive.

```
Stage 1: Syntax normalization
  small, local de-minification rewrites
  e.g. UnminifyBooleans (!0 → true), SimplifySequence, UnBracketNotation

Stage 2: Transpiler helper unwrapping + module-system reconstruction
  Babel/TypeScript helper recovery and require() → ESM, ending with UnEsm
  e.g. UnInteropRequireDefault, UnObjectSpread, UnWebpackInterop, UnEsm

  UnEsm also recovers narrowly proven same-module CommonJS reads: a later
  `module.exports` read may use the sole stable default binding, and a later
  `exports.name` read may use a uniquely assigned stable export binding. Direct
  method calls additionally require a receiver-insensitive function. Earlier
  `undefined` export declarations are permitted; multiple value writes, later
  resets, computed writes, value escapes, receiver-sensitive calls, early
  reads, direct eval, and hoisted function declarations remain fail closed.

  ── cross-module barrier (unpack only: fact collection + late pass) ──

Stage 3: Structural restoration
  expression/operator-level syntax recovery
  e.g. UnTemplateLiteral, UnNullishCoalescing, UnOptionalChaining

Stage 4: Complex pattern restoration
  multi-statement/control-flow pattern recovery
  e.g. UnIife, UnParameters, UnJsx, UnEs6Class, UnRegenerator, UnAsyncAwait

Stage 5: Modernization
  idiomatic-ESNext rewrites of already-correct code
  e.g. VarDeclToLetConst, ArrowFunction, ObjShorthand, UnForOf

Stage 6: Cleanup and renaming
  inlining, rename recovery, import/export cleanup, optional DCE
  e.g. SmartInline, SmartRename, ImportDedup, DeadDecls/DeadImports (optional),
  plus most repeat passes (UnIife2, UnJsx2, SmartRename2, ...)
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
   resolved, bundler-normalized AST directly. Apply exact normal-only rewrites
   backed by detector-owned runtime facts, then run the rule registry through
   `UnEsm`, clone that barrier AST for webpack factory-IIFE fact recovery, and
   extract import/export facts. Retain the pre-recovery AST together with its
   `Globals` and unresolved mark.
2. **Phase 2 (parallel):** Resume the retained Phase 1 AST → cross-module late
   pass (re-export consolidation, namespace decomposition, fact-aware helper
   recovery) → run the registry range resuming after `UnEsm`, through `UnReturn` →
   targeted late cleanup/recovery → emit.

The late pass uses facts from Phase 1 to inform cross-module rewrites (e.g.,
repairing a proven CommonJS object or callable-property edge, converting
`ns.foo` to `import { foo }`, or recognizing a split helper module). Facts are
extracted in `crates/core/src/facts.rs` and consumed by
`crates/core/src/provider_import_repair.rs`,
`crates/core/src/namespace_decomposition.rs`,
`crates/core/src/reexport_consolidation.rs`, and fact-aware rules. See
[fact-system.md](fact-system.md) for details.

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
  wakaru/
    src/
      lib.rs                        — stable compiler-like public façade
      decompile.rs                  — owned single-file operation adapter
      unpack.rs                     — Vec and incremental UnpackJob operations
      source.rs, options.rs         — owned inputs and private builder options
      output.rs, error.rs           — artifacts, reports, diagnostics, fatal errors
      debug.rs, sourcemap.rs, vue.rs — standalone auxiliary namespaces

  cli/
    src/
      main.rs                       — CLI entry point (clap)

  core/
    src/
      lib.rs                        — internal engine exports
      driver.rs                     — internal driver consolidation
      driver/
        single_file.rs              — decompile() orchestration
        unpack.rs                   — unpack(), unpack_raw(), and multi-module pipeline
        trace.rs                    — rule trace orchestration and formatting
        diagnostics.rs              — post-transform diagnostic warning collection
        discovery.rs                — internal structural-detection helper
        output.rs                   — internal path normalization and dedup helpers
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
        browserify.rs               — browserify-family splitter (incl. Cocos Creator 2.x)
        closure_module_manager.rs   — Closure ModuleManager/gstatic splitter
        systemjs.rs                 — System.register splitter + ESM reconstruction
        esbuild.rs                  — esbuild/Bun splitter (CJS factories + scope-hoisted)
        amd.rs                      — AMD define() bundle splitter
        wrappers.rs                 — UMD/AMD wrapper unwrapping for detection retry
        metro.rs                    — Metro plain-bundle detection and extraction
        scope_hoist.rs              — heuristic scope-hoisted splitting (esbuild, Bun, Rollup, Vite)
      utils/
        paren.rs, swc_safety.rs     — paren stripping, SWC panic guards
    tests/
      common/mod.rs                 — test helpers (see docs/testing.md)
      *_rule.rs                     — per-rule unit tests
      *_unpack.rs                   — per-bundler unpack/pipeline snapshot tests
                                      (webpack4 + raw, webpack5 chunk, bundle_unpack
                                      = webpack5 + browserify, Closure ModuleManager,
                                      esbuild, systemjs, amd, metro, rollup, bun,
                                      multi-file)
      webpack_fixtures.rs           — generated webpack4/5 + ncc fixture coverage
      cocos_creator_unpack.rs       — Cocos 2.x detection + dependency-map coverage
      noop_pipeline.rs              — stability tests
      snapshots/                    — insta snapshot files

  wasm/
    src/
      lib.rs                        — wasm-bindgen entry point (decompile + unpack)

docs/
  architecture.md                   — this file
  public-api.md                     — supported Rust façade contract and design decisions
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
