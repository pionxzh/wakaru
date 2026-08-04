# Stable Public Paths for Processed Inputs

Status: **ADOPTED — Option A.** Implementation promotes the existing synthetic
entry to the processed input's stable public path. Option B is rejected as a
correctness mechanism: direct consumer rewrites are out of scope, and an
export-owner map may exist only as internal facade validation if needed. This
addresses normal unpack output only. `--raw` is splitter passthrough and is not
a correctness surface for this proposal.

## Summary

When a physical ESM input is selected for processing, Wakaru can
replace its original filename with `entry.js` plus generated chunks. Other
inputs in the same job still refer to the original relative path. A
development-corpus audit found this failure class repeatedly: sibling inputs
kept static links to a processed input's original content-hashed path (of the
`./index-<hash>.js` shape) after that input became `entry_3.js` plus chunks.

The adopted design is **a stable facade at the processed input's original
public path**, implemented by making the existing synthetic entry that
facade/entry. It already retains the original module declarations and residual
evaluation code. Wakaru does not synthesize a separate re-export-only facade.

Do not implement exact export-owner rewriting. It is sound for a proven named
or default import, but it has no general one-module target for namespace
imports, `export *`, side-effect imports, or `import()`. An export-owner map may
exist internally only when needed to validate the facade surface.

## Applicable formats

The invariant is format-agnostic: it applies whenever one physical input was
addressable by a relative ESM path from a sibling input in the same multi-input
job and processing would otherwise remove that path. Two current paths meet
that definition:

- top-level heuristic scope-hoist splitting of ESM input; and
- structural esbuild unpack of an ESM code-split chunk when the detector emits
  a synthetic `entry.js` for that input.

Reserve the public path by `PreparedInputId`, not inside either splitter. Both
formats then acquire the same boundary and filename-ordering behavior without
duplicating policy in `scope_hoist.rs` and `esbuild.rs`.

Webpack, Browserify, Metro, SystemJS, and AMD are explicitly out of scope.
Sibling files address their outputs through format-specific runtime loading,
not relative ESM imports to the physical bundle filename. Closure
ModuleManager is likewise a script-loaded shared-namespace format rather than
an ESM public-path case.

## Current boundary loss

The top-level scope-hoist splitter deliberately folds every original
`ModuleDecl` into its synthetic entry and emits that entry as `entry.js`; it
promotes or synthesizes exports on chunks only when another generated cluster
needs their bindings. See `extract_clusters` and `emit_clusters` in
[`scope_hoist.rs`](../../crates/core/src/unpacker/scope_hoist.rs). This gives
Wakaru an internal module graph, but it drops the physical input's public path.

The recursive splitter already applies the stronger invariant: its synthetic
entry keeps the detected parent module's filename, while child chunks are
namespaced beneath the parent stem. See `namespace_scope_hoisted_split` in
[`scope_split.rs`](../../crates/core/src/driver/unpack/scope_split.rs). The
top-level case should acquire the same public-boundary property.

The structural esbuild unpacker independently reserves and emits a synthetic
`entry.js` for residual ESM declarations and evaluation code. In a code-split
multi-input job, a sibling chunk can still import the physical esbuild chunk by
its original relative path. That synthetic entry therefore needs the same
promotion even though its module boundary came from structural detection.

Multi-input preparation currently deduplicates all emitted filenames and then
builds an old-to-final filename map per `PreparedInputId`. The rewrite updates
static imports, re-exports, dynamic imports, and unresolved string `require`
calls *within that physical input*. See `prepare_multi_source_modules` in
[`merge.rs`](../../crates/core/src/driver/unpack/merge.rs) and
`rewrite_import_sources` in
[`filename_recovery.rs`](../../crates/core/src/driver/unpack/filename_recovery.rs).
That is the correct scope for collision repair, but it cannot infer that a path
from input A to input B should now target one of B's generated outputs. The
missing concept is B's stable public module boundary, not another filename
guess.

## Semantic requirements

For each processed input that was addressable as a relative ESM module before
splitting, normal output must preserve these invariants:

1. The same public relative path resolves after processing.
2. Named and default imports remain indirect live bindings, not copied values.
3. Namespace consumers observe exactly the public exports of that input,
   including `default` when present, and no splitter-only exports.
4. Named re-exports and `export *` retain the original resolution and ambiguity
   behavior.
5. A side-effect-only import evaluates the transformed input graph once, with
   its entry effects and dependency ordering retained.
6. `import()` continues to load the public module path and fulfills with that
   module's namespace object after evaluation; computed specifiers must not
   require static edge discovery.
7. If Wakaru cannot prove the boundary, it preserves the input as one processed
   module at its public path instead of emitting a partial facade or guessing an
   owner.

These are ESM requirements rather than output-style preferences. During module
linking, named/default imports resolve to immutable indirect bindings and fail
when the requested export is missing or ambiguous. See ECMA-262's
[`CreateImportBinding`](https://tc39.es/ecma262/2026/multipage/executable-code-and-execution-contexts.html#sec-createimportbinding)
and [module
initialization](https://tc39.es/ecma262/2026/multipage/ecmascript-language-scripts-and-modules.html#sec-source-text-module-record-initialize-environment).
Module namespace objects are exotic objects whose properties are derived from
the module's unambiguous exports and read through `ResolveExport`. Dynamic
import returns a Promise and, after loading and evaluation, fulfills it with
the selected module's namespace. See ECMA-262's
[`ResolveExport`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-scripts-and-modules.html#sec-resolveexport),
[`GetModuleNamespace`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-scripts-and-modules.html#sec-getmodulenamespace),
[module namespace exotic
objects](https://tc39.es/ecma262/2026/multipage/ordinary-and-exotic-objects-behaviours.html#sec-module-namespace-exotic-objects),
[`ModuleRequests`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-scripts-and-modules.html#sec-module-semantics-static-semantics-modulerequests),
[`InnerModuleEvaluation`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-scripts-and-modules.html#sec-innermoduleevaluation),
[`EvaluateImportCall`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-expressions.html#sec-evaluate-import-call),
and [`ContinueDynamicImport`](https://tc39.es/ecma262/2026/multipage/ecmascript-language-expressions.html#sec-ContinueDynamicImport).

## Option A — stable facade under the public path

The facade owns the logical filename by which other inputs address this
processed input. Its dependencies point to the final generated outputs.

### Preferred form: reuse the synthetic entry

For either applicable format, rename the existing synthetic entry from
`entry.js` to the input's safe logical output path and reserve that path before
generated chunk names are assigned. This is an entry/facade rather than a thin
barrel:

- original import/export declarations are already folded into it;
- bindings moved into chunks are already imported back when the entry refers to
  them;
- original residual statements and side effects stay on the public evaluation
  path;
- no extra module record is inserted into cycles or top-level-await ordering;
- namespace and dynamic-import consumers see the entry's real ESM namespace,
  rather than a JavaScript object synthesized by Wakaru.

This form needs a public-path plan and filename reservation, but it does not
need to infer an export owner for every public name.

### Rejected fallback: a thin generated facade

A separate facade is not part of the adopted implementation. It would require
both:

- a complete inventory of the input's public exports, distinguished from
  splitter-synthesized cross-cluster exports; and
- a proven execution entry whose dependency preserves the original side effects.

The facade would use explicit indirect exports, including an explicit
`default` re-export when applicable, and would retain the execution entry as a
dependency when the re-export owners alone do not do so:

```js
import "./internal-entry.js";
export { default } from "./chunk-default.js";
export { status, start } from "./chunk-service.js";
```

Blanket `export * from "./internal-entry.js"` is not sufficient. `export *`
does not forward `default`, and the current splitter may add exports solely for
generated cross-cluster imports. Forwarding all of those would enlarge the
public namespace and could change downstream `export *` ambiguity. A thin
facade therefore requires public-versus-synthetic export provenance that entry
reuse does not. If the existing entry cannot safely own the public path,
Wakaru instead preserves the input as one processed module at that path.

### Behavior by consumer form

| Consumer form | Facade behavior |
|---|---|
| `import { value } from "./input.js"` | An explicit re-export or the retained entry resolves to the defining binding, preserving liveness. |
| `import value from "./input.js"` | Same, but `default` must be explicit; star forwarding is insufficient. |
| `import * as ns from "./input.js"` | The runtime creates the facade/entry's real module namespace object. Correctness requires its public export set to be exact. |
| `import "./input.js"` | The facade depends on the execution entry, so the transformed graph evaluates once even when no export is requested. |
| `export { value } from "./input.js"` | The consumer keeps its source unchanged and links through the stable boundary. |
| `export * from "./input.js"` | The language's normal `ResolveExport` rules retain omissions and ambiguity, provided the facade surface is exact. |
| `import("./input.js")` | Literal and computed paths still select the public module; fulfillment yields its namespace after its dependency graph evaluates. |

Adding a distinct facade node would introduce risk in cycles and
top-level-await graphs. The adopted entry reuse avoids that extra module record.

## Rejected Option B — exact export-owner map and direct rewrites

This rejected design records, for each processed input, a mapping such as
`(public path, exported name) -> (final output filename, final exported name)`
and rewrites consumers directly:

```js
import { status } from "./input.js";
// becomes
import { status } from "./chunk-service.js";
```

The map must follow binding identity through splitting and all normal-output
renames; matching emitted strings after the pipeline is not enough. It must
track the requested external export name independently of the consumer's local
alias, and it must be calculated against final deduplicated filenames.

This is attractive for a static named/default import or named re-export whose
owner is unique: it avoids an extra hop while preserving an indirect live
binding. It does not generalize to the rest of ESM:

| Consumer form | Direct owner rewrite limitation |
|---|---|
| Named/default import | Sound only with one proven owner and final external export name. Missing or multiple owners must not be guessed. |
| Namespace import | Several outputs may own the namespace's names. Redirecting to one owner loses names or exposes internal ones; combining owners into an ordinary object loses module-namespace semantics and live resolution. |
| Side-effect import | There is no exported name from which to choose an owner. Rewriting to one chunk can omit entry effects or change evaluation order. |
| Named re-export | Has the same narrow unique-owner case as a named import. |
| `export *` | Rewriting to multiple owners can expose synthetic exports and create or erase ambiguity; rewriting to one owner drops exports. |
| Dynamic import | Redirecting to one owner changes the namespace and module identity. `Promise.all(...).then(() => ({ ... }))` is not equivalent to dynamic import of a module namespace. Computed specifiers cannot be exhaustively rewritten. |

A mixed strategy that rewrites only the edges it understands does not solve
the dangling-path defect: every unrevised namespace, side-effect, star, or
dynamic edge still needs the original path. Once that stable path exists, the
facade is already the correctness mechanism and direct rewrites are merely an
optional optimization.

Direct consumer rewrites remain out of scope. A binding-aware owner map may be
used internally only if facade-surface validation later needs it.

## Ambiguous and missing owners

There are two distinct kinds of ambiguity:

- **Source-semantic ambiguity**, such as colliding `export *` names. The
  transformed boundary should preserve it. It must not choose an arbitrary
  defining module; an explicit request for that name is supposed to fail
  during linking, while an unrequested ambiguous star name is omitted from a
  namespace.
- **Wakaru uncertainty**, where provenance or a later rename no longer proves
  which output owns a public binding. This is a transformation veto, not an
  invitation to approximate the source.

For the preferred reused-entry facade, original module declarations continue
to let the ESM linker resolve source-semantic ambiguity, and most public names
do not need an owner decision. For a thin facade or direct owner rewrite, every
required public name must resolve to exactly one final binding. A missing owner,
multiple candidate owners, an unknown public export surface, or an unproven
execution entry must cause per-input fallback to one processed module at the
public path. Do not emit a partial facade, leave selected consumers dangling,
or suffix the public path and hope consumers follow it.

If two physical inputs claim the same normalized public path, that is path
ambiguity rather than an export ambiguity. Wakaru must diagnose it before
splitting those inputs and preserve a non-split result or fail according to the
existing operation's collision policy. Renaming one facade to `_2.js` would
break whichever consumers meant that input.

## Filename-dedup interaction

The filename collision rewrite already shipped remains useful, but public
paths need to be planned before generated names:

1. Derive the safe normalized public path for every retained physical input and
   associate it with `PreparedInputId`.
2. Reject duplicate public-path claims in the same case-sensitivity domain used
   by output filename deduplication. Ambiguity fails closed; never suffix a
   public path.
3. Reserve all public paths before naming entries and chunks. A generated
   module colliding with a public path is renamed; a public facade is not.
4. Give the reused entry the reserved public path. Namespace
   its generated chunks beneath that path's stem where practical, matching the
   recursive scope-split design.
5. Run global deduplication for generated outputs, then apply the existing
   per-input old-to-final rewrite to the facade/entry and its chunks. This keeps
   all internal import, re-export, dynamic-import, and supported string
   `require` references aligned with final filenames.
6. Do not add cross-input owner guesses to the filename rewriter. Consumers
   continue to target the unchanged public path. Pass the reserved path set to
   `build_rename_map` so later Sentry-based readability recovery cannot rename
   a public facade away.

This ordering avoids a regression of the previously fixed collision bug. In
particular, a facade must be generated against logical module identities and
then have its internal specifiers rewritten to final deduplicated paths; it
must not bake pre-dedup chunk strings into final output.

## Implementation validation

Use synthetic multi-input fixtures only. Normal-output tests should cover:

- named and default imports, including a reassigned exported binding to prove
  liveness;
- namespace import with `default`, exact key coverage, and no synthesized
  cross-cluster exports;
- side-effect-only import with a once-only counter;
- named re-export, `export *`, and an intentionally ambiguous pair of star
  exports;
- literal and computed dynamic import, checking namespace keys and repeated
  import identity;
- two processed inputs whose generated chunk names collide, proving the
  existing dedup rewrite updates facade internals while both public paths stay
  unchanged;
- duplicate normalized public paths, proving Wakaru falls back or reports the
  ambiguity rather than suffixing a facade;
- a Sentry-annotated facade paired with an ordinary annotated module, proving
  readability recovery leaves the reserved facade untouched; and
- both applicable producers: a synthetic top-level scope-hoisted ESM input and
  a synthetic structural esbuild ESM chunk input.

The graph validator should assert target existence and requested-export
availability on normal output. Raw passthrough may help localize a failure, but
raw-only diagnostics are outside this proposal's acceptance criteria.

## Decision

Adopt **Option A**, specifically by promoting the existing synthetic entry to
the stable public facade path for top-level scope-hoist and structural esbuild
ESM inputs. It preserves a single addressable ESM boundary, handles every
consumer form without discovering all consumer sites, retains the language's
own live-binding and ambiguity machinery, and minimally changes the current
splitter graph.

Do not build direct consumer rewrites or a thin facade. A binding-aware
export-owner map may exist only as supporting evidence to validate that a
public binding remains reachable. When entry reuse is unprovable, preserve one
processed module at the public path instead of approximating the boundary.

## Adjacent plain-input path issue

`filename_for_fallback_input` currently flattens a plain input to its basename.
That can break cross-directory references between two plain inputs in a
multi-input job, even though neither input was split. Public-path reservation
does not naturally repair that case: the adopted mechanism promotes a proven
synthetic entry, while a plain input already is its own entry and loses path
information earlier during fallback naming. Covering plain inputs would require
a separate safe common-root policy for retaining their relative directory
structure. That issue remains documented but out of scope for this change.
