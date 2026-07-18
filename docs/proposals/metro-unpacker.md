# Metro (React Native) Unpacker

Status: **IMPLEMENTED (plain-bundle v1).** Numeric and string module IDs,
array and indexable-object dependency maps, minified factory parameters,
prefixed `__d` globals with Metro's unprefixed `__r` startup call, dynamic
dependency-map preservation, multiple entries, raw/full unpack, prepared-AST
handoff, and provenance are covered. Indexed/file RAM bundles and Hermes
bytecode remain out of scope.

See also: [architecture.md](../architecture.md) for the unpacker dispatch and
the extraction-normalization boundary, [testing.md](../testing.md) for the
per-bundler test pattern.

## What Metro is

Metro is the default bundler for React Native (and Expo). Its plain output is a
single-file JavaScript bundle built from three runtime primitives:

- `__d(factory, moduleId, dependencyMap)` — **d**efine a module.
- `__r(moduleId)` — **r**equire/run a module (the entry kick-off).
- `__c()` — clear the module registry (occasionally present).

A defined module's factory has a fixed 7-parameter signature:

```js
__d(function (global, require, _$$_IMPORT_DEFAULT, _$$_IMPORT_ALL, module, exports, dependencyMap) {
  "use strict";
  var utils = require(dependencyMap[0]);       // dependency by index
  Object.defineProperty(exports, "__esModule", { value: true });
  exports.greet = function () { return utils.hi(); };
}, 0, [1]);                                    // moduleId 0, depends on module 1
```

Key structural facts an unpacker can rely on:

- **Module id** is the second `__d` argument — numeric by default, occasionally
  a string when `createModuleIdFactory` names them.
- **Dependency resolution is indexed**: inside the factory, `require(dependencyMap[N])`
  (or a minified alias like `_$$_REQUIRE(dependencyMap[N])`) resolves local
  index `N` to the real module id via the third `__d` argument. This is the
  Metro analogue of webpack's numeric `require(42)` and browserify's dependency
  map — the same class of rewrite the existing extractors already do.
- **Interop** is explicit: `importDefault`/`importAll` params wrap CJS/ESM
  interop, similar to Babel/webpack interop that later pipeline rules handle.
- **Entry**: the bundle ends with one or more `__r(<entryId>)` calls.

## Why this fits wakaru cleanly

Metro is structurally close to formats wakaru already unpacks. The extractor is
"webpack4-shaped": a registry of factory functions keyed by numeric id, with an
indexed dependency map to rewrite into `require()` specifiers. The
transpiler-helper and ESM-recovery pipeline that runs after extraction is
**format-agnostic** and needs no Metro-specific work — Metro modules are
Babel-transpiled CJS/ESM, exactly what Stage 2+ already recovers.

The extractor's job is the bundler-coupled boundary work only (per the
architecture's raw-unpack contract): split each `__d`, rename the fixed factory
params to conventional names, and rewrite `dependencyMap[N]` accesses to real
module specifiers. It does **not** run a slice of the rule pipeline.

## Scope

**In scope (v1):**

- Plain (non-indexed) Metro bundles: top-level program of `__d(...)` calls
  followed by `__r(...)`.
- Numeric and string module ids.
- Indexed dependency-map rewriting: `require(dependencyMap[N])` →
  `require("<resolved-id>")`, mapping to the emitted per-module filename the
  same way webpack4 numeric ids are mapped.
- Factory param normalization (the 7-arg signature → conventional names).
- `--raw` extraction (registry split + boundary normalization only) and full
  `--unpack` (extraction → normal decompile pipeline), matching every other
  format.

**Out of scope (later, or never):**

- **RAM / indexed bundles** (the binary `.bundle` format with a module table
  header and per-module string segments). This is not text JS; it needs a
  binary parser, not the AST unpacker. Separate proposal if demanded.
- **Inline-requires** reconstruction beyond what the standard pipeline gives —
  Metro's `inlineRequires` optimization moves `require` calls to first use;
  recovering hoisted top-of-module imports is a nice-to-have that the existing
  import-dedup/hoist rules may already partially cover. Measure before building.
- Naming modules from Metro's `serializer` path output — treat any preserved
  path comments as filename hints only (same policy as the Bun path comments in
  the esbuild unpacker), never as module boundaries.

## Detection

Add `Metro` to `BundleFormat` and a `metro::detect_from_module` following the
`amd.rs` / `browserify.rs` template. Detection signal (all must hold, to avoid
matching unrelated code that happens to call a `__d`):

1. Top-level program is predominantly `__d(...)` expression statements.
2. At least one `__d` call has the shape `__d(<function|arrow>, <id>, <array?>)`.
3. Presence of a `__r(...)` entry call **or** the `__d` factory's 7-param
   signature (the signature is distinctive enough to disambiguate from AMD's
   `define`).

Place detection in the `detect_bundle_candidate` chain in `unpacker/mod.rs`.
Order: after webpack/browserify/systemjs/esbuild and before the AMD fallback —
Metro's `__d`/`__r` markers are specific, so position is not delicate, but
keeping it before AMD avoids any `define`-vs-`__d` ambiguity in UMD-wrapped
edge cases. First match wins, as with all detectors.

## Extraction plan

Mirror `webpack4.rs`'s structure (it is the closest existing extractor):

1. Collect every `__d(factory, id, depMap)` into a registry `{ id → (factory,
   depMap) }`. Once a definition prefix is selected, every top-level call using
   that exact `${prefix}__d` runtime must parse; otherwise reject the whole
   candidate rather than emit a partial module table with dangling imports.
2. For each module, take the factory body as the module source. Rename the
   fixed params (`global`, `require`, `importDefault`, `importAll`, `module`,
   `exports`, `dependencyMap`) to conventional names via the existing
   extraction-normalization helpers (factory param rename is already a named
   boundary helper used by webpack/browserify).
3. Rewrite dependency access: `require(dependencyMap[N])` →
   `require("<filename-for-resolved-id>")`, resolving `N` through the module's
   `depMap` array to a real id, then to that id's emitted filename. Handle the
   minified require alias by matching the call target that is applied to
   `dependencyMap[...]`, not the literal name `require`.
   Dynamic import, prefetch, maybe-sync, and `resolveWeak` shapes retain a
   synthesized local copy of the complete dependency map (including `paths`)
   when indexed accesses remain after static rewriting. Reject extraction when
   such a factory-only reference has no map value to preserve.
4. Normalize `importDefault`/`importAll` call sites. Declaration initializers
   become default/namespace imports directly because the Metro runtime loader
   identity proves the original import kind; uncommon expression-position
   calls fall back to `require(...).default` / `require(...)` so the normal
   pipeline can continue processing them.
5. Emit `UnpackResult::new(modules, BundleFormat::Metro)` plus the private
   prepared-AST sidecar; keep webpack-style
   ESM markers untouched so the decompile pipeline recovers live exports.

Provenance byte ranges: record each module's factory-body span, same as the
other extractors emit for `provenance.json`.

## Reproduce first

Per [rewrite-assumptions.md](../rewrite-assumptions.md), start from real Metro
output, not a hand-written mock:

```bash
npx react-native@latest init MetroRepro   # or: npx create-expo-app
# produce a plain (non-RAM) release bundle:
npx react-native bundle --platform android --dev false \
  --entry-file index.js --bundle-output repro.bundle --reset-cache
```

Also generate a **minified** variant (Metro runs Terser in release) and a
**dev** variant (unminified, with more whitespace and module comments) — the
extractor must handle both. Add small hand-reduced fixtures for unit tests, but
validate against the real bundle. Follow the synthetic-id rule for committed
tests: use obviously-fake module ids, never values copied from a real app.

## Testing

- `crates/core/tests/metro_unpack.rs` — detection + decompiled-output snapshots,
  following `bundle_unpack.rs`.
- Raw-unpack snapshots for the extraction/normalization boundary (the
  dependency-map rewrite and param rename), following `webpack4_unpack_raw.rs`.
- A noop/negative case: a plain script that calls a user function named `__d`
  must **not** be detected as Metro (guards the detection signal).
- Pinned Metro 0.87 dev and minified bundles live under
  `crates/core/tests/bundles/metro-gen/` and in `wakaru-fixtures`, extending the
  cross-bundler regression net with producer-generated output.

## Resolved questions and remaining boundary

1. **Metro runtime import loaders need boundary handling.** They implement
   cached default/namespace interop rather than Babel helper calls, so the
   extractor recovers their import kind before the generic pipeline. Ordinary
   Babel helper modules still flow through Stage 2 normally.
2. **String module IDs are supported.** Sanitized string IDs become output
   paths and indexed dependencies are rewritten relative to those paths.
3. **RAM bundles** remain demand-gated. They are the harder, binary format;
   defer until asked, then spec separately.
