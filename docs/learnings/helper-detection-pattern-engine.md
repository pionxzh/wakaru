# Learning: don't try to replace helper detection with a generic pattern matcher

**TL;DR — Do not rebuild helper detection on a corpus matcher, an ast-grep-style
DSL, or a skeleton-pattern + checker engine expecting to shrink the code. It was
tried, measured end-to-end against real bundles, and reverted. The detection
code is large because the problem is irreducibly semantic, not because the
matchers are written badly. ~93% of detection is marker-based or stateful and
cannot be expressed as a fixed pattern; the fixed-shape remainder is too small
(~10–14 of ~209 functions) for a shared engine to pay for itself. If you are
here because "helper detection has too many lines," read this before starting —
the wall is structural, not a missing abstraction.**

## The recurring idea

Helper/transpiler-pattern detection lives in `transpiler_helper_utils.rs` (~13
body-shape matchers) **plus** ~190 more detection functions inside the rules
(`un_es6_class`, `un_regenerator`, `un_async_await`, `un_object_rest`,
`un_object_spread`, `un_class_fields`, `un_jsx`, …): **~209 functions / ~17.5k
LOC** total. It is repetitive — lots of `let-else` pyramids and match-arm
tree-walking — so it is natural to think: "express helpers declaratively
(patterns / a DSL / vendored helper sources) and collapse the boilerplate."

This was investigated thoroughly. Three approaches, each prototyped in shadow
mode and measured against a real helper-bearing bundle corpus:

## What was tried and what it measured

1. **Exact corpus matching** (canonicalize a candidate — alpha-rename bound idents,
   normalize, print — and string-compare against vendored helper sources).
   - Result: exact parity, zero false positives, **only on small structurally-stable
     helpers** (`interopRequireDefault`, `classCallCheck`, after fixes `objectSpread`
     and non-inlined OR-dispatcher forms). Hopeless on `interopRequireWildcard`,
     `objectWithoutProperties`, and any **fully-inlined-sub-helper** body.
   - Ceiling ≈ 22% of the matcher core, and only the *cheap* matchers. Rejected.

2. **ast-grep-style metavariable / relational matching.** Strictly more expressive
   than (1), but: exact patterns don't unify `if/else` vs ternary (different
   nodes); metavariables match by text and **lose `SyntaxContext` binding identity**
   (the precision that prevents false positives); and relational `has`/marker rules
   are just a *different notation* for the marker-based logic the bespoke matchers
   already implement. No new capability on the hard helpers. (ast-grep is also
   tree-sitter-based; it can't run mid-pipeline on SWC ASTs.)

3. **Skeleton-pattern + semantic-checker engine** (build a minimal SWC-native
   pattern engine; pattern handles navigation + binds holes to `(sym, ctxt)`,
   a small checker verifies residual semantics). This is the *right* architecture
   in principle — it removes navigation boilerplate, not semantics — and it was
   built, productionized, and used to migrate 4 matchers with **zero behavior
   change**:
   - `classCallCheck` 54→21 LOC (−61%), `interopRequireDefault` 92→54 (−41%),
     plus `possibleConstructorReturn`, `assertThisInitialized`.
   - But a full classification of all ~209 functions showed the migratable
     subset caps at **~10–14 matchers / ~1.1–1.4k LOC** — the other ~12k LOC is
     structurally incompatible. The engine (~350 LOC + tests) **net-increased**
     total LOC and never reached the scale where a shared abstraction justifies
     its dual-paradigm cost. The engine, the corpus matcher, and the shadow
     harness were all **removed** afterward; only this learning remains.

## Why the wall is structural (the root cause)

A pattern/skeleton can only express a helper whose **discriminating structure is
at fixed positions**. The bulk of wakaru's detection is not like that:

- **Marker / signal-accumulation** (`scan_stmts_for_markers` and friends): "does
  the body contain `Array.isArray` *and* `Symbol.iterator` *somewhere*",
  "`Object.assign` + `.apply(this, arguments)` anywhere". `slicedToArray`,
  `toConsumableArray`, `objectSpread`, `objectWithoutProperties`, `extends`,
  `interopRequireWildcard`, `un_class_fields`, `un_jsx`. A fixed skeleton is
  strictly *less tolerant* than a marker scan, so migrating it changes behavior.
- **State machines**: `un_regenerator` (~4k LOC), `un_async_await` — recognition
  is a stateful traversal of switch/case/label/try structure, not a shape.
- **Recursive / compositional**: helpers whose sub-helpers get inlined produce
  combinatorially many bodies (the same reason corpus matching failed).

These are large *because the recognition is genuinely semantic and
variance-tolerant*, which is exactly what makes wakaru robust across transpiler
versions and minifiers. The lines encode irreducible knowledge; no matching
notation removes them.

The fixed-shape helpers that a pattern engine *can* handle are the simple, mature,
slow-changing ones — a small minority. New detection work in this project is the
*hard* kind (new bundlers, new minifier behaviors), which is marker-based and
wouldn't use the engine. So the abstraction's reach never grows.

## What to do instead

- **Keep helper detection bespoke.** Per-helper hand-written matchers using
  `MatchContext` (binding-slot identity), `helper_matcher.rs` (binding lifecycle),
  and `expr_utils.rs` (structural compare) are the right tool. The size is the
  cost of the problem.
- **Real, banked reductions came from de-duplication, not from a new engine:**
  consolidating inline-vs-declaration detection into one place removed ~260 lines
  with no behavior change (see git history / the proposal doc). That kind of
  targeted dedup is where line savings actually live.
- **If you only need to recognize one new fixed-shape helper**, just write the
  matcher — it's ~20–90 lines and consistent with everything around it. Do not
  introduce an engine for it.

## When this conclusion would change (and only then)

Revisit a shared engine **only** if a future change makes the migratable set
reach ~15–25+ matchers — e.g. if a large new family of *fixed-shape* helpers
appears, or if the marker-based matchers are themselves restructured so their
discriminating signals become positional. Neither is true today. Absent that,
the dual-paradigm and maintenance cost outweighs the benefit.

## Evidence / pointers

- Full investigation and per-phase data: `docs/proposals/helper-detection-unification.md`.
- Design rationale for the bespoke approach: `docs/helper-detection.md`.
- The experiment built a shadow-mode comparison harness (a candidate matcher run
  beside the production detector over real bundles, reporting disagreements). It
  was removed with the rest of the experiment, but the approach is easy to
  reconstruct from git history if a future detection change needs validating.
