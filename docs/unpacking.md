# Unpacking and Module Boundaries

Read this for detector changes, factory normalization, scope-hoisted
splitting, or raw/multi-input unpack behavior. Start with
[architecture.md](architecture.md) for the shared pipeline; use
[fact-system.md](fact-system.md) for the phase barrier and consumers of
detector-owned facts, and [cli.md](cli.md) for user-facing options.

The dispatch implementation in `crates/core/src/unpacker/mod.rs` defines
detection order. Bun executable container parsing is a separate intake layer;
see [bun-standalone.md](bun-standalone.md).

## Detection order and supported shapes

Each unpacker detects a specific bundle format and extracts individual modules.
The payload may include a prepared AST as described below. Detection is
attempted in order — first match wins:

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

## Vercel ncc

Vercel ncc CommonJS output with an IIFE webpack bootstrap is handled as a
webpack5 producer, not as a separate bundle format. Its module table is
extracted normally, while the statements beginning at the binding ultimately
assigned to `module.exports` become a synthetic `entry.js`.
`__nccwpck_require__` calls are normalized to `require()` and numeric module
IDs are rewritten to the emitted module filenames. This recovers the
JavaScript module graph; files emitted separately by ncc's asset relocation
loader are not reconstructed by the unpacker. ncc's `.mjs` output uses a
top-level runtime rather than this IIFE shape and is not structurally split.

## Browserify and Cocos Creator

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

## Factory normalization and failure boundaries

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

## Scope-hoisted splitting

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

## Detector payload and runtime facts

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

## Production-build scope

Development builds are a non-goal. Wakaru targets shipped, production
bundles; artifacts that only appear in dev-mode output — such as webpack's
`devtool: 'eval'` family, which wraps every module body in an `eval("...")`
string for fast rebuilds — are intentionally not recovered. Such bodies pass
through as-is rather than being unwrapped. Don't propose eval-string
unwrapping or other dev-build-only recovery work.

## Driver intake and raw output

The function names below refer to internal/test-support adapters. The
supported Rust surface is `wakaru::unpack` / `UnpackJob`; see
[public-api.md](public-api.md).

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
