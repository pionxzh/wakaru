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
   runtime-only entry files, an IIFE bootstrap whose own table is empty
   because production builds concatenated the dependency-free app code into
   the runtime (accepted only with the proven require lifecycle and an
   extracted entry; the unwrapped `output.iife: false` form still needs a
   non-empty table), inline startup (both webpack's own
   `var __webpack_exports__ = {}` form and Vercel ncc's variant), and the
   unwrapped `output.iife: false` form. `experiments.outputModule` bundles that
   carry top-level ESM `export`/`import` declarations are left untouched — their
   public surface can't yet be recovered faithfully
2. **webpack4** — `(function(modules) { ... })([...])` with `__webpack_require__` runtime
3. **webpack5 chunk** — JSONP chunk push with a webpack module object
4. **Turbopack** — Next.js 15.3+ production chunks: the client
   `globalThis.TURBOPACK` push and the server `module.exports` payload. See
   [Turbopack](#turbopack) below
5. **browserify family** — numeric-keyed
   `(function e(t,n,r) { ... })({1:[function(...){...}, {...}], ...})`,
   including Cocos Creator 2.x's string-keyed `window.__require` variant
6. **Closure ModuleManager** — Google/Closure shared-namespace module segments,
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
7. **SystemJS** — top-level `System.register(...)` modules
8. **esbuild / Bun** — scope-hoisted ESM namespace boundaries
   (`__export(ns, ...)`) and CJS factory helpers (`__commonJS` / `__esm`).
   Bun's bundler emits the same helper shapes as esbuild, so CJS-interop
   bundles from Bun are detected and split by this unpacker.
   Preserved Bun path comments are used only as filename hints for modules
   already found through structural helper patterns; they are not module
   boundaries by themselves. Bindings that stay in the synthetic entry (a
   declaration the last-module boundary search leaves in the remainder, or a
   restored lazy-module namespace object reached through the `import()`
   lowering `then(() => (init_x(), ns_x))`) are exported from `entry.js` and
   imported by the scope modules that reference them. The edge is added only
   where the resulting entry/module cycle provably cannot observe the
   entry's initializers: every consumer reference sits inside a function or
   class bound to a top-level name unreachable from eager or unknown-timing
   references. Reachability propagates through guard dependencies and namespace
   exports until a fixed point: calling `start` which references `read` exposes
   `read`'s dependencies too. Recursive groups with no early or unknown root
   remain deferred. Unclassified references, including adopted support shapes
   other than hoisted function declarations, count as unknown timing.
   Alternatively, the owner may be a hoisted function whose transitive
   references stay within such functions
   and external imports. A reference whose timing
   cannot be proven (a callback flowing into an eager expression, an
   object-literal method or getter at top level, a computed key, an
   immediately invoked body, a locally called function), an eager read of
   entry state, or a write keeps the unlinked reference, which `debug
   validate` reports as `unresolved_reference`; relocating such state is not
   attempted. The guard analysis does not infer callback scheduling: an eagerly
   reached function containing a Promise callback can therefore retain an
   unlinked reference even though that callback runs later at runtime.
   Mutable state and hoisted support functions that write it share one
   synthetic owner. A standalone factory whose body assigns a top-level
   binding directly is a writer of that state as well: every factory writing
   the same binding joins one group, the group declares the binding once, and
   the entry declaration moves with it so entry reads become imports. An
   entry function declaration that writes the group's state joins the unit
   the same way when its binding is never reassigned and its other
   dependencies already have an emitted owner; entry calls it through an
   import. Any other entry writer either relocates as a statement or cancels
   the split: the group demotes back into entry, a CommonJS factory as a
   synthesized cached callable and a lazy ESM initializer as a guarded init
   function. Compatibility forwarding files are discarded with a demoted
   owner; entry imports of factory-owned bindings are synthesized only after
   final ownership is known, so no link to a cancelled split survives.
   When a scope module or merged factory still references the
   demoted group, demotion is impossible and the entry writer's assignment
   to the imported state remains as a residual that output validation
   reports.
   Grouping joins standalone factories only. When a scope module owns the
   state, a factory that writes it merges into the scope module even if it
   also writes entry state no module claimed: the scope module adopts that
   state with its declaration, and a factory writing only adopted state
   merges on a later round. Adoption needs a movable top-level declaration
   and no other entry writer, since an entry write would become an
   assignment to an import. A factory that cannot merge stays standalone
   without declaring the scope module's state, so its write stays unlinked
   and output validation reports it as `unresolved_reference`.
   Standalone CommonJS factories participate in that grouping
   alongside lazy ESM initializers; each retains its own callable wrapper and
   cache/initialization guard. A CommonJS factory that assigns top-level
   state also preassigns that state like a lazy ESM initializer does: a scope
   module that writes the same state claims the factory and emits it as an
   exported cached callable, and any other module that calls the factory
   imports it from its owner. Synthesized cache and guard names avoid the
   names already declared in the module they land in and every identifier
   the factory body mentions, since a body local or parameter would shadow
   the helper inside the callable. An unconsumed hoisted entry function can also
   move into a scope module when every binding it writes belongs to that module
   and its other dependencies are itself, that module's bindings, or existing
   imports. A writer reached only from entry may also retain read-only entry
   dependencies through imports: adopted hoisted functions receive the same
   deferred-body profile as ordinary declarations, so the existing timing guard
   checks these edges per consumer. Write checks include the complete adopted
   declaration, even when its target is entry-owned rather than factory-owned.
   Those writes retain the existing unlinked boundary; avoiding an import write
   does not by itself recover shared entry state. An unsafe sibling does not
   suppress a safe consumer's import or gain permission to import the same
   binding. Entry initializers stay in place. Writers reached from extracted
   modules with entry-owned dependencies, or writers spanning owners,
   are not adopted by this path; property mutations and shadowed locals do not
   count as writes to the exported binding. Functions whose own binding is
   reassigned stay in entry, avoiding a new imported-function write.
9. **Metro** — React Native/Expo plain-JavaScript bundles made of top-level
   `__d(factory, moduleId, dependencyMap)` definitions and `__r(entryId)`
   startup calls. The extractor resolves indexed dependencies, normalizes the
   fixed seven factory parameters, and recovers Metro's default/namespace
   import loaders. When dynamic import, prefetch, maybe-sync, or `resolveWeak`
   leaves dependency-map accesses in the extracted module, the full map is
   preserved as a local binding. Every definition using the selected runtime
   prefix must parse before any modules are emitted, preventing partial tables
   with dangling imports. Indexed/file RAM bundles and Hermes bytecode are
   separate binary formats and are not handled by this detector.

If nothing matches directly, `wrappers.rs` unwraps UMD factory, AMD
`define()`, and whole-file synchronous zero-argument IIFE wrapper shapes and
retries the same detection chain on each unwrapped candidate. The plain-IIFE
path covers esbuild/Rollup browser output, including a single named-global
declaration such as `var app = (() => { ... })()`. A terminal `return expr`
retains `expr` as an evaluated startup statement. Named function expressions,
function-only binding observations (`this`, `arguments`, or `new.target`),
parameters, async/generator wrappers, nested function-level returns, and
top-level siblings retain their original boundary. Finally, **AMD** (`amd.rs`)
detects files consisting of top-level `define(id, deps, factory)` calls and
splits each define into a module.

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

### Webpack 5 trailing startup calls

A trailing IIFE is extracted as webpack's entry wrapper only when it occupies
the whole startup region, optionally preceded by a standalone unused empty-object
anchor or a canonical `__webpack_exports__ = {}` anchor used exclusively by
webpack export helpers. A live anchor passed to application code or captured by
a closure retains its declaration and wrapper, even with the canonical name. Entry declarations and side effects before
an authored trailing call remain in `entry.js`, including entry expressions
merged into the last runtime sequence. Terser can also merge the wrapper itself
into that sequence (`r.d = ..., r.o = ..., (() => { ... })()`); the wrapper at
the end of the sequence is unwrapped like a standalone one. Raw extraction uses
the same boundary.

Terser can inline a single-use require function into the startup and call it
with the entry id (`!function r(id) { ... }(100)`). Its body is runtime, never
entry code: a bare or `!`-prefixed call marks module 100 as the entry. When a
library build consumes the call (`window.lib = function r(id) { ... }(100)`),
`entry.js` gets webpack's unrolled form,
`var lib = require("./module-100.js"); window.lib = lib;`, which `UnEsm` turns
into an import. This happens only when the assignment targets name globals; a
target that reads a bootstrap binding or `this` would change meaning outside
the bootstrap, so that statement is still dropped.

Wrapper removal requires an anonymous synchronous, non-generator function or
synchronous arrow, with no parameters or call arguments. Async and generator
calls retain their invocation boundaries; an async IIFE must not turn into
top-level await. Doing so would make module evaluation wait for an unawaited
call and turn that call's rejection into a module-evaluation failure. This
restriction applies to raw extraction as well as normal unpacking.

## Turbopack

Turbopack production chunks register factories into a runtime shared by
every chunk of the app. The client form is
`(globalThis.TURBOPACK || (globalThis.TURBOPACK = [])).push([script, ...])`,
also with a computed `globalThis["TURBOPACK_…"]` global; the server form is
`module.exports = [...]`. From Next.js 15.5 the payload is runs of numeric ids,
each followed by one factory; extra ids before a factory share it (see merged
groups below). Next.js
15.3–15.4 used `(G = G || []).push([script, { id: factory }])`, 15.4 also
`id: [factory, [aliasId]]`. Beside the containers a file may hold only
expression statements. The debug-id polyfill that `turbopack.debugIds`
(Next.js 16+) prepends to every client and server chunk is matched by its
exact text and dropped: it only records the chunk's id in
`globalThis._debugIds`. Any other statement runs when the chunk loads and
belongs to no factory, so those statements are emitted together, in source
order, as an entry module `prelude.js`; their order relative to the container
is not kept. A runtime registration (`{ otherChunks, runtimeModuleIds }`)
holds no modules.
The server form, which has no distinctive global, must be the only statement
besides the debug-id polyfill, and is accepted only when a factory calls a
module-protocol member on its first parameter.

Each factory runs as `factory(ctx, module, exports)`. The detector translates
it into webpack's `(module, exports, require)` form and prepares it with the
webpack 5 normalizer, so the existing webpack ESM and CommonJS recovery
applies. Only members with a known meaning are translated:

- `ctx.r(id)` and `ctx.i(id)` become `require(id)`; a computed id stays a
  runtime `require(expr)`.
- `ctx.s([...])` becomes `require.r(exports)` plus `require.d` getters and
  value assignments. Next.js 16 encodes a value as `name, 0, value`; 15.5 and
  the 16 server output use `name, getter`; 15.3–15.4 pass `{ name: getter }`.
  A setter is not translated. A second argument naming the factory's own id
  is the current module; one naming another id is a merged group.
- Merged groups: Turbopack merges modules into one factory, lists their ids
  before it, and registers each module's exports with `ctx.s(bindings, id)`.
  The runtime runs the factory once and each registration fills that id's
  module-cache entry, so an id a registration names without listing it is a
  module too, which other factories (and the group itself) read through the
  cache. The factory becomes the module of its first listed id. It exports
  the other members' bindings under aliases (the exported name, or
  `name_<id>` when that name is `default` or already taken), and each other
  member becomes a facade module of live `export { alias as name } from`
  re-exports, so `ctx.i(member)` resolves to the facade. A group is accepted
  only when every registration is a top-level `ctx.s` with an id, every
  listed id has one, no exported name repeats within a member, and no
  unlisted member is another factory's id or another group's member;
  otherwise the factory stays opaque. Extra ids before a factory whose
  registrations name no other id stay aliases of its first id.
- `ctx.v(x)`, `ctx.n(x)`, and `ctx.q(url)` become `module.exports = x`, as a
  statement or wherever the call's result is discarded (a UMD branch, a `&&`
  right operand, the expression body of a discarded arrow IIFE). `ctx.q`
  exports an asset URL; the runtime's deployment suffix is not modeled. A
  value export whose result is used is not translated.
- `ctx.m` becomes the module parameter. All three parameters get fresh
  spellings first, so synthesized references cannot be captured. From Next
  15.4.0, `ctx.e` is Turbopack's compile-time replacement for a free
  top-level `this` in a CommonJS module, and nothing else compiles to it.
  Outside any function, constructor, class field, or static block it becomes
  `this` again, so TypeScript's `(this && this.__name) || …` helper guards
  stay recognizable and `UnEsm` keeps a module that uses its top-level `this`
  CommonJS. Below those it stays the exports parameter.
- A generated async loader module becomes
  `module.exports = () => Promise.resolve().then(() => require(target))`.
  `ctx.A(loader)` (15.5+) and `ctx.r(loader)(ctx.i)` (15.3–15.4) inline that
  expression when the loader is in the same input, and otherwise call
  `require(loader)()`, which the multi-input numeric rewrite resolves when the
  loader's chunk is also an input.
- `ctx.r` read without a call is dropped where its value is discarded (the
  AMD branch of a UMD wrapper reads it in a comma expression) and otherwise
  becomes the require parameter, as when Turbopack's `define` wrapper passes
  it to the factory the way webpack passes `__webpack_require__`.
  `ctx.r.bind(ctx)`, the bound form App Router server page entries pass, also
  becomes the require parameter; a residual member bound the same way stays
  `__turbopack_context__.<letter>.bind(__turbopack_context__)`.
- `ctx.f(map)`, the runtime's `require.context` that dynamic `import()` and
  `require()` with a template request compile to, becomes a call to a local
  `moduleContext` function inserted at the top of the module: a copy of the
  runtime implementation (call, `keys`, `resolve`, `import`). Each entry's
  `module: () => ctx.r(id)` or `ctx.A(id)` is translated in place, so the
  require stays inside its thunk and loading stays lazy. From 16.1 the
  runtime drops a `?query` or `#fragment` from the request before the
  lookup. The copy does the same unless the chunk is a 15.2–15.4 object
  container; 15.5–16.0 flat chunks cannot be told apart from 16.1 ones and
  get the newer behavior, which differs only for a request that contains
  `?` or `#`. A call with a constant key listed in an object-literal map
  (Next.js resolves its instrumentation hook this way) becomes that entry's
  `module` body directly. A factory with a local `Object` or `Error` binding
  stays opaque, since the copy reads those globals.
- `ctx.x("name", () => require("name"))`, a server external, becomes
  `require("name")`; `ctx.g` becomes `globalThis` when no local binding has
  that spelling.
- Runtime members without a module-graph meaning, called or read as a
  value, are kept as `__turbopack_context__.<letter>`, and the module gets a
  non-error `runtime_residual` diagnostic: chunk loading (`ctx.l`, `ctx.L`),
  path and file URL resolution for the `import.meta.url` emulation (`ctx.P`;
  `ctx.F` from 16.3), the host `require` (`ctx.t`), and the throwing require
  stub (`ctx.z`). These letters kept their meaning from 15.3 through 16.4
  canary. They reach no module graph or export, so the module still
  recovers; the residual name is undefined in the split output. A factory
  that already uses that spelling stays opaque.
- 15.3–15.4 factories destructure `{ g, __dirname, m, e }` from `ctx`. The
  `m`/`e` bindings become fresh module/exports parameters, and a body that the
  minifier wrapped in a block is spliced to the top level when its
  declarations cannot collide.

Any other use, such as `ctx.j`, async modules,
a worker or chunk base path (`ctx.b`), a read of
`__dirname`, or the context escaping as a value, keeps that factory opaque
with a `decompile_failed` diagnostic. Runtime letters changed meaning
across releases, so an unknown member is never guessed. A translated factory
that the webpack normalizer cannot rename to `module`/`exports`/`require`,
such as an AMD branch that reads a free `module`, stays opaque on its own
with a `webpack_factory_recovery_failed` diagnostic.

Turbopack can inline a module into its importer, and those boundaries are
not recoverable. The strict-mode factory groups, `ctx.S` re-exports, and the
inverted `ctx.s` tag of unreleased 16.4 canary builds are rejected; a canary
container keeps the original fallback.

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
renamed without changing host-environment lookup. Browserify/Cocos and Metro
reject the candidate in that case, and normal fallback preserves the original
bundle; webpack isolates the factory as described below.

Webpack and translated (Turbopack) factories make one exception for a free
`require`: when every free `require` is a direct call whose first argument is
neither a number nor a string naming a module id, the loader parameter takes
the name `require` and the two merge. The output already treats `require` as
the host require (externals become `require("name")`), and a bundler leaves a
free `require` only where the author opted out of bundling
(`__non_webpack_require__`, `turbopackIgnore`), so the merged call is the
author's own text. In a browser chunk the original free call throws, while
the output resolves it. A free `require` used as a value, a member, or under
`typeof`, and any free `module` or `exports`, still counts as a capture.

Webpack has a narrower partial-failure path for two factory-local failures. A
minified factory may reuse its `module`, `exports`, or loader parameter as an
ordinary local after its last runtime use, and a factory may read one of the
canonical names free (webpack emits a free `require` for
`__non_webpack_require__` and keeps `typeof exports` free in ESM modules). In a
numeric-ID container, when the reuse boundary cannot be proved or the rename
would capture a free reference, Wakaru preserves that factory's extracted body
unchanged and marks only that module as failed; other factories in the same
structurally proven container remain recoverable. Named-ID containers keep the whole-input fallback
because an unresolved path-like runtime call could otherwise be mistaken for an
ESM import. A fixed-point pass
removes failed factory IDs from the rewrite map before retrying dependants, so
calls to an opaque factory follow the existing absent-ID behavior instead of
becoming invented ESM edges. The opaque body never enters rule processing,
fact collection, filename recovery, or recursive scope splitting. Other
normalization failures still reject the whole container, except in factories
another detector translated (Turbopack), which fail one at a time. A container
with no recoverable factory still uses the original whole-input fallback.

For a provable reuse boundary, localization runs before webpack's ordinary,
position-insensitive runtime normalization. Only immediately evaluated uses
before the first unconditional write, plus supported loader uses in that
write's initializer, receive the canonical `module`, `exports`, or `require`
identity. The write is lifted to a new `var` local and every later use follows
that local. Contexts inside a prepared module follow detector conventions; the
driver re-derives them with the resolver at the Phase 1 handoff, so rules that
classify identifiers by `unresolved_mark` see the local as a module-local
binding rather than an undeclared global. Runtime-helper members and mapped
module calls in a loader prefix can then use the normal webpack recovery path; post-write calls
and members stay attached to the new local value even when a numeric argument
happens to match a module-table ID. Webpack 5's
`module = require.hmd(module)` / `nmd(module)` decorators return the module
they receive, so they are runtime-preserving operations, not lifetime
boundaries. The normalizer reduces each one on the factory's own `module`
binding to a plain `module` read in any expression position, such as
`(module = require.nmd(module)).exports = ...` or an IIFE argument, and drops
the read in top-level statement position. A decorator on a shadowing inner
parameter keeps its write. A
first real write may be a top-level assignment, a `var` redeclaration of the
factory parameter, or a direct element inside a top-level/initializer sequence
when splitting the sequence preserves its evaluation result. A consumed alias
reset on the guaranteed-once right-hand side of a top-level `for ... in` is
also supported by replacing the reset with the localized value before lifting
its initializer. An exact `module.exports = exportsParameter = value` bridge,
including one element of a top-level comma sequence, keeps the assignment chain
in place and introduces an uninitialized local for the parameter's second
lifetime. This bridge requires a declared, unwritten default binding and one
whole-value export assignment, with no module-object escape or direct eval.
Complex initializers and unproven defaults remain opaque so later ESM recovery
cannot erase properties observable through the alias. Proven named property
writes stay on the default object as well as becoming named exports.
Numeric calls
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
surface proof. A variable-held synchronous UMD factory can also recover its
default when its sole call uses the generated `.call(exports, require, exports, module)`
arguments and its anonymous, parameterless body does not observe that invocation
context. The call stays in place; its undefined-result guard and initial empty
export object are preserved. Reassignment, escape, direct eval, `with`, and
other unmodeled CommonJS runtime references keep the original form.
Generated AMD callbacks whose complete body returns a captured local binding
can also lose their `.call(exports, require, exports, module)` or
`.apply(exports, [])` shell. The binding read remains at the original call site,
and the undefined-result guard is retained; no stable-function or non-undefined
inference is required. When this eliminates the complete `exports`/`require`
surface, initialization-time `module.exports` reads and whole-value writes can
use one fresh local initialized to `{}`. Conditional paths, repeated writes,
and immediately invoked bodies stay in place, followed by one ordinary CJS
assignment for the existing ESM recovery pipeline. Deferred runtime references,
module-object escape/reassignment, dynamic scope, and slot calls/tags/deletes
reject the candidate. An enclosing `.call(this)` is removed only for an
anonymous synchronous parameterless function that cannot observe its invocation
context. This is not general CommonJS runtime emulation or a mixed-format
output mode.
Recursively split children inherit neither detector fact.
Detector output may also carry a private per-module failure sidecar. The normal
driver turns it into an operational diagnostic plus
`ModuleStatus::DecompileFailed` while preserving the raw extracted body; raw
detector APIs may discard this metadata because raw output has no graph-quality
contract.

## Output source maps

`--emit-source-map` maps each unpacked module back into its input. Phase 2
re-parses every module's extracted text, so the Phase 2 emitter only knows
positions in that intermediate text. The missing hop comes from extraction:

- Extractors that print a module record emitter points (extracted offset →
  input offset) when the driver passes `SourcePositions::Record`. Pieces
  assembled by string concatenation go through `MappedCode`, which shifts
  each piece's points by the length before it; synthesized glue text has no
  points.
- A module whose code is a verbatim slice of the input (plain inputs, opaque
  webpack factories, whole-source fallbacks, unlowerable SystemJS registers)
  records the slice start in `verbatim_source_offset` instead.
- Nested scope-split children compose their points through the parent's
  offsets (`InputOffsets::compose`). The import-specifier rewrites that
  follow the split are applied as edits that shift later points and drop
  points inside the replaced text.
- A SystemJS dynamic-export register re-unpacks a printed inner bundle. That
  outer hop is recorded in every mode so prepared inner modules' points
  always target the real input.

Phase 2 maps each emitter position through those offsets and converts the
input offset with a per-input UTF-16 line index. Only exact points map;
anything without one stays unmapped. Each output position keeps a single
mapping, the innermost node's (the single-file builder shares this rule;
see `add_innermost_mappings`). The line index is built only when maps
are requested. Unpack maps name the input and omit `sourcesContent`, which
would repeat the whole input in every module's map.

Requesting maps must not change the unpacked code or provenance. Nested
scope splitting therefore keys its ESM-recovered attempt and child
provenance on `UnpackedModule::mapped_in_every_mode` (points recorded by
prepared-module materialization and Closure emission, which happen in every
mode), never on whether points are present. Recording points costs emitter
bookkeeping per token, which is why extraction discards them by default.

## Known gaps

- **Browserify drops the code around the bundle.** When the input's top
  level has other statements before or after the prelude call
  (`(function(){function r(e,n,t){...}return r})()({1: [...]}, {}, [1])`),
  only the table modules are written; the statements around it appear in no
  output file, and nothing reports it. webpack keeps authored trailing calls
  in `entry.js` (see [Webpack 5 trailing startup calls](#webpack-5-trailing-startup-calls));
  the fix is to do the same here, or at least warn. Whether the AMD,
  SystemJS, Closure, and Metro unpackers lose code the same way is
  unchecked.
- **Relative `require` inside a top-level expression.** A module binding
  assigned inside an expression (`r = f((e = require("./m")).x)`) is not
  turned into an import. The `require` stays in the ESM output, where it
  throws, and no warning reports it. rspack production entries emit this
  shape.
- **rspack inline `require.n` getter.** An inline `require.n` getter called
  in place in an rspack (1.7) entry (`(() => e && e.__esModule ? e.default :
  e)()`) is not collapsed: `UnWebpackInterop` matches only a getter bound to a
  declared variable. webpack 5 builds of the same source do not have this
  shape. (rspack's version metadata, `require.rv` and `require.ruid`, is
  treated as runtime and stays out of the entry.)
- **A kept `require` of a provider recovered as ESM.** A relative `require`
  that stays a call (after an `import_hoisting_eagerness` barrier, or nested
  in a function) returns the provider's module namespace once that provider
  becomes ESM. Two consumer meanings break:
  - the whole value of a `module.exports = v` provider, now `export default
    v`: `const n = require("./m"); n()` throws (`hypothetical`);
  - an interop default whose helper `UnWebpackInterop` already removed
    (`require.n(x).a`), so the binding is read as the default:
    `n.usesSpy()` throws (producer `webpack@4.47` concatenation + Terser,
    with an `swc@1.16` CommonJS dependency).

  The suggested fix is a Phase 2 pass: when the provider's facts prove
  ESM output and the consumer's binding means its default, rewrite the
  call to `require("./m").default`. The whole-value case needs a provider
  fact that the CommonJS value is the default export (`module.exports = v`,
  not `exports.default = v`). The interop case needs a Phase 1 consumer
  fact, collected before the interop helpers go, like
  `whole_require_sources`. Keeping the provider CommonJS does not fix the
  interop case, because the `__esModule` object still has to be unwrapped.
- **A `.cjs` name still gets ESM syntax.** A single-file decompile keeps a
  `.cjs`/`.cts` input CommonJS (`run_un_esm` stands down), but unpack does
  not apply that gate. Two inputs reach it: a `.cjs` file passed to
  `--unpack` (several inputs, or one that is not a bundle), and a recovered
  module whose id keeps its `.cjs` name (producer `webpack@5.111.1`
  `optimization.moduleIds: "named"` + Terser). Either way a CommonJS module
  comes out as `export default ...` in a `.cjs` file, which Node fails to
  load. Lifting the gate alone is not enough: a harmony module still calls
  `require.r`/`require.d` until `UnEsm` removes them, so the fix has to
  decide per module, from what the bundle says it is, whether to keep it
  CommonJS.

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
module set: filenames are made unique before fact collection, and numeric
webpack module IDs are mapped to those final filenames so entry/chunk
references can be rewritten across physical input files.

A numeric reference only links within its own build. One page often loads
several unrelated builds (an app, an ad SDK, a support widget), each numbering
its modules from its own table. Each input's build identity is the
chunk-loading global it pushes into (`(self.webpackChunk_app =
self.webpackChunk_app || []).push(…)`, Turbopack's `globalThis.TURBOPACK…`) or,
for a runtime, the one it binds to a local (`var n = self.webpackChunk_app =
self.webpackChunk_app || []`). An input that names a global matches only
inputs sharing a name; inputs that name none (CommonJS chunks and their
runtime, server chunks, a self-contained bundle) match only each other. A
reference is rewritten when exactly one candidate of its build carries the ID,
so the same ID in an unrelated build no longer blocks the rewrite, and two
candidates in one build stay ambiguous and keep the numeric call.
In normal output, opaque factories still reserve their numeric IDs but cannot
become rewritten callers or targets. Recoverable siblings remain eligible for
cross-input rewrites; an opaque factory does not disable its whole container.

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
