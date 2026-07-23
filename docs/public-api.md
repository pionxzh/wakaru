# Public Rust API

The supported behavioral contract of the `wakaru` façade lives in the crate's
rustdoc (`cargo doc -p wakaru --no-deps`): exact signatures, per-item
semantics, defaults, and failure behavior are documented on the items
themselves, and the crate-root examples are compile-checked doctests. This
document records what rustdoc is the wrong place for: the design decisions,
compatibility boundaries, and internal invariants behind that surface.

## Publishing model

The public façade is published as `wakaru`. Cargo cannot package a façade with
an unpublished path dependency, so `wakaru-core` is also published as a
lockstep, exact-version implementation dependency. It remains explicitly
unsupported as an integration surface and may change whenever the façade
version changes.

## Goals

- Expose Wakaru as a compiler-like service, not as an SWC transformation
  toolkit.
- Keep parsed and prepared modules internal until final output requires text.
- Detect and parse each input once in the normal path.
- Represent partial recovery explicitly instead of requiring callers to infer
  it from warning strings.
- Keep inputs, module artifacts, recovered artifacts, provenance, diagnostics,
  and detection reports structurally associated.
- Leave room to add formats and diagnostics without another breaking release.

## Surface overview

Two root operations — `decompile(Source, DecompileOptions)` and
`unpack(Vec<Source>, UnpackOptions)`, with `UnpackJob` as unpack's
incremental-intake form — plus optional namespaces: `bun` (single-file
executable extraction), `debug` (normalize, rule tracing, rule metadata),
`sourcemap` (embedded-source extraction), and `vue` (experimental standalone
SFC recovery). Root operation options carry `RecoveryOptions`; enabled
framework recovery returns framework-neutral `ArtifactOutput` values alongside
the primary JavaScript modules. See rustdoc for everything else.

## Design decisions

- Publish the stable façade as `wakaru`; publish `wakaru-core` first as its
  exact-version, unsupported implementation dependency.
- Owned `Source` inputs: passing an owned `String` lets Wakaru move it into
  SWC's source storage instead of imposing a `source.to_string()` copy.
- Use private option fields with builder methods so options can grow without
  breaking callers; result types are `#[non_exhaustive]`.
- One `ModuleOutput` module artifact type for both operations; single-file
  decompile uses `EntryStatus::Unknown` and empty provenance rather than a
  second single-file-only module type.
- Framework recovery is additive: `ArtifactOutput` associates unique,
  normalized output files with module indices and explicit kind/status values;
  recovery failure leaves the JavaScript output intact and emits a diagnostic.
- Keep `InputReport::module_indices`, because synthesized modules can have no
  provenance.
- Keep the failure-oriented name `ModuleStatus::DecompileFailed`; it always
  carries at least one diagnostic.
- Use `DiagnosticCode::WebpackFactoryRecoveryFailed`
  (`webpack_factory_recovery_failed`) when one structurally identified webpack
  factory is preserved as opaque while its container's other modules continue.
- `Error` is reserved for fatal operation failures; recoverable per-module
  problems are `Diagnostic` + `ModuleStatus`. SWC error types and
  `anyhow::Error` are never part of the public contract.
- Keep `Vec<Source>` on the convenience function for a simple bindable
  signature.
- Add `UnpackJob` for incremental intake. A `Vec`-only API cannot bound
  directory-walk memory: every candidate string is already resident before
  Wakaru gets a chance to drop skipped inputs. Both forms delegate to the same
  detection and processing implementation.
- Return `InputReceipt` from `UnpackJob::push` so detection progress is
  available during intake.
- Evaluate `UnmatchedInput::Error` at `finish`, preserving operation-level
  failure semantics while keeping the job usable after every successful push.
- Represent heuristic scope-hoist recovery as
  `InputDetection::HeuristicScopeHoisted` instead of pretending it identified
  a bundler; `BundleFormat` stays structural-only.
- Keep Inspect evidence context separate from module provenance:
  `ModuleOutput::inspection_context` repeats the pre-cap scope-hoist component
  spans on unambiguous fine siblings, while `provenance` continues to describe
  only the bytes attributed to that output module. Empty context is the normal
  case and does not imply missing module provenance.
- CLI defaults may differ from library defaults (the CLI selects
  `DceMode::TransformOnly`; the library default is `Off`).
- Filesystem path validation, output-directory writes, filename collision
  handling, and source-map extraction to disk remain CLI responsibilities.
- Tracing remains the observability mechanism for phase and per-rule timings,
  but span names, fields, and nesting are instrumentation details, not a
  stable contract.

### Framework recovery

Vue does not establish a per-framework end-to-end composition pattern.
Supported root workflows use `RecoveryOptions` on `decompile` and `unpack`,
and return framework-neutral `ArtifactOutput` values. Angular component
recovery is the first root workflow using this contract.

The common artifact model supports multiple files rather than assuming every
framework produces one source file. Artifacts associate themselves with module
indices, following `InputReport`, and guarantee unique normalized filenames.

The current experimental Angular path analyzes the finalized JavaScript
modules together as one owned module workspace. That preserves cross-module
role evidence and avoids any bundler-specific analyzer route, but it currently
parses that finalized workspace once after materialization. Moving the analyzer
onto retained prepared ASTs is a performance optimization and must not change
the artifact contract or make a bundler own framework semantics.
Caller-supplied import resolution remains useful only for standalone namespace
operations such as `vue::recover`.

Private option fields and non-exhaustive result types allow the integrated
surface to grow without a breaking change. Future framework namespaces may
provide framework-specific standalone recovery and option types, but should
not add `svelte::decompile`, `angular::decompile`, or equivalent composition
entry points.

## Not public

The following remain private implementation details:

- `rules` and every individual rule visitor
- `facts` and cross-module fact maps
- `unpacker` and detector-specific entry points
- `UnpackedModule`, `UnpackResult`, and prepared AST types
- namespace decomposition and re-export consolidation passes
- TDZ visitors and SWC-facing source-map rename helpers
- SWC AST, `Mark`, `SyntaxContext`, `SourceMap`, and visitor types
- output path and filesystem-oriented helpers

Rule names and stable rule metadata remain available through `debug`; rule
execution and AST mutation do not. Production visibility should not be
determined by the test layout: integration tests that need internals belong
crate-local, not behind widened `pub`.

## Internal processing boundary

This is not public API, but it is a required architectural invariant for the
public operations above. Every input becomes a `PreparedInput` (id, detection,
modules); every module carries a `ModulePayload` that is either a prepared AST
or source text. A plain input is a `PreparedInput` containing one module; a
structural or heuristic bundle contains zero or more extracted modules. All
selected modules cross the same fact-collection and phase-2 boundaries;
detector-specific work ends before that boundary, and the payload only
controls how the common pipeline obtains its initial AST. There is no
separate public or phase-level webpack route.

The executor returns one internal record per module, with code, output source
map, and provenance kept together. Provenance identifies its physical input
with a typed prepared-input ID; input identity is never encoded into a
filename or sentinel string for a later adapter to decode.

### Performance invariants

1. Each physical input is detected at most once per `unpack` call.
2. A compatible plain JavaScript input is parsed at most once before rules.
3. A prepared module is not emitted and reparsed before the rule pipeline.
4. `UnmatchedInput::Skip` does not require a separate detection preflight.
5. `UnpackJob::push` releases a skipped candidate's source before accepting
   the next candidate. Peak intake memory is therefore bounded by retained
   detected/processed inputs rather than every file visited by a directory
   walk.
6. Text is materialized only for final output, explicit raw output, or a
   recovery fallback.
7. Source-map modes may take an explicitly slower path when source-coordinate
   state cannot safely cross the parallel boundary.
8. All output ordering is deterministic regardless of Rayon scheduling.

These invariants prevent the normal rewrite pipeline from reintroducing the
emit/parse round trips removed by the prepared-AST work. Experimental framework
recovery may parse the finalized owned module workspace once until it can
consume retained prepared ASTs directly. The normal-path invariants are
enforced by span tests in `wakaru-core` (see `docs/fact-system.md` and the
driver tests).
