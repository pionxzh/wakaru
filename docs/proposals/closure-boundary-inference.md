# Closure ModuleManager: Segment-Boundary Inference

Status: **PROPOSED, evidence-gated.** The current conservative behavior passes
the focused and external fixture gates, and no reproducible producer false
negative is known. A passing corpus gate does not prove that fallback never
hid a rejected Closure candidate. Phase A (internal diagnostics only) is worth
doing on its own; Phase B must not start until it produces evidence that a real
producer emits a currently-rejected shape. Phase C is rejected.

Ground rules: follow [AGENTS.md](../../AGENTS.md), including a focused unit
test for every change. Use synthetic module ids and filenames in tests and
commits — never values copied from private fixture bundles — and update
affected documentation (`docs/architecture.md` describes the current
conservative contract) in the same commit that changes behavior.

## Current behavior

`collect_segment_candidates` in
`crates/core/src/unpacker/closure_module_manager.rs` accepts exactly one
segment shape: a top-level `try { … } catch (e) { shared._DumpException(e) }`
statement, optionally preceded by `/*_M:id*/` markers in the whitespace gap
before it. Segment identity requires agreement between up to three signals:

- the marker id (`markers_before_statement`),
- the loader boundary inferred from paired `before("id")` / `after()` helper
  calls inside the try block (`infer_loader_boundary`),
- for markerless bundles, positional order proven against the initializer
  graph (`assign_segment_ids` / `proven_response_order`).

Everything else rejects the whole candidate (the file then goes through the
normal single-file pipeline, so the cost of a false negative is an unsplit
but correct output):

1. any non-try statement after the preserved prefix,
2. a try statement without the `_DumpException` handler,
3. markers followed by a non-try statement,
4. non-blank text before or between markers in a gap,
5. a marker id disagreeing with the inferred boundary id,
6. duplicate segment ids, or markerless candidates whose positional ids
   cannot be proven for every segment.

## Design observation

Markers and try-shape are entangled today, but they are different-strength
signals. A standalone, cleanly parsed `/*_M:id*/` marker is an authoritative
byte position for that segment start. When every emitted logical segment is
marked, those positions establish the complete partition without requiring
the following statement to have any particular shape. The
try/`_DumpException` wrapper is then semantic corroboration. The statement
walk currently treats the try shape as primary and markers as annotations on
it, which is why rejections 1 and 3 exist even when markers establish complete
coverage.

The principled loosening, if evidence ever calls for it, is to invert that only
for bundles with complete marker coverage: slice segments by marker byte
ranges and downgrade the try/helper structure to a consistency check. A mixed
marked/markerless response is not complete coverage and stays on the current
strict path. Markerless bundles also keep the strict rules — there the
try/helper/graph agreement is the only signal, and an unguarded statement
genuinely has no provable owner.

## Phase A — rejection diagnostics (no behavior change)

A rejection today is a silent `None`; nobody can tell which rule fired.
Introduce a private rejection-reason enum (for example
`UnguardedStatement { index }`, `MarkerBeforeNonTry`, `BoundaryIdMismatch`,
`UnprovenPositionalOrder`) and expose it only through a test helper or opt-in
`tracing::debug!` event. Do not add warnings, CLI output, or a public API for
this investigation. Record reasons only after a strong Closure anchor (a valid
initializer, or a standalone marker corroborated by the dump-exception guard)
has matched, so ordinary JavaScript does not flood the tally with irrelevant
near misses.

Run that instrumentation over the fixture corpus and any available
Closure-produced applications, and tally which anchored rejections occur on
real bundles. That tally is the backlog for Phase B; an empty tally means
Phase B is not built.

## Phase B — marker-authoritative slicing (evidence-gated)

Only for shapes Phase A proves real, one shape per commit, each with a
synthetic fixture reproducing the shape. Require every emitted logical segment
to begin with a standalone marker, with complete coverage corroborated by the
initializer/served-order metadata when available. Stacked markers continue to
represent explicit empty modules. A response with only some segments marked
does not enter this path.

After proving complete coverage, parse each byte range from marker N to marker
N+1 as that segment's body regardless of whether it contains one statement, a
minifier-fused sequence, or multiple statements. Preserve the existing outer
wrapper and provenance model, and keep any observed try/helper boundaries as
consistency checks. A slice that cannot be parsed as JavaScript rejects the
whole candidate.

Invariants that must not loosen, with their guarding tests:

- embedded marker text is not a boundary
  (`marker_text_embedded_inside_a_comment_is_not_a_boundary`),
- forward graph indexes reject the candidate
  (`rejects_forward_graph_indexes`),
- a try without the dump-exception guard is not a segment
  (`rejects_marker_without_dump_exception_guard`),
- duplicate ids reject, and marker/boundary id disagreement rejects,
- markerless bundles keep every current rule.

## Phase C — markerless loosening: rejected

Without markers there is no byte-position authority, so accepting unusual
shapes means guessing statement ownership. A wrong guess mis-attributes code
across module boundaries, which is worse than not splitting. This phase is
recorded only so the next reader does not re-derive it.

## See also

- [metro-unpacker.md](metro-unpacker.md) — the same evidence-first pattern
  applied to Metro; indexed/file RAM bundles and Hermes bytecode remain out
  of scope there.
- `docs/architecture.md` — the Closure section documents the current
  conservative contract; keep it true in the same commit as any change here.
