# Proposal: Unified helper detection — expression-site registry + pattern-as-JS matching

Status: **CLOSED / mostly rejected.** Phase 1 (inline-vs-declaration detection
de-dup) landed and stuck — a real ~260-line reduction with no behavior change.
Everything past it (corpus matching, then a skeleton-pattern + checker engine)
was prototyped in shadow mode, measured against real bundles, and **reverted**:
the migratable subset is ~10–14 of ~209 detection functions, far below the scale
a shared matcher needs, and the engine net-increased LOC. The detection code is
large because the problem is irreducibly semantic (~93% marker-based or stateful).
**Read [../learnings/helper-detection-pattern-engine.md](../learnings/helper-detection-pattern-engine.md)
for the conclusion and don't re-run this.** The engine, corpus matcher, and the
shadow-mode comparison harness built during the investigation were all removed
afterward (they validated a rejected approach); reconstruct from git history if
ever needed. The sections below are the original proposal, preserved for context.

## Problem

Helper detection works (see [helper-detection.md](../helper-detection.md)) but
its cost has outgrown its design. Measured against the current tree (numbers
approximate, attribution by function ranges):

- `rules/transpiler_helper_utils.rs` is ~4,900 lines, of which **~77%
  (~3,760 lines) is hand-written body-shape matchers** — 13 predicates covering
  17 `TranspilerHelperKind` variants. Two dominate: the slicedToArray family
  (~1,280 lines including inline-form extraction) and asyncToGenerator
  (~770 lines). The infrastructure around them (`LocalHelperContext`, lifecycle
  utilities, import-path classification) is comparatively small and healthy.
- **Six rules carry ~1,000 additional lines of private detection**:
  `un_es6_class` (~400), `un_sliced_to_array` (~250), `un_object_spread`
  (~150, esbuild-specific), `un_object_rest` (~100),
  `un_interop_require_default` (~80), `un_class_call_check` (~50).
- The duplication has one dominant shape: **the central scanner only detects
  helpers as module-level declarations, but rules also need to recognize the
  same helpers at expression sites** (inline IIFE/arrow forms). The interop,
  classCallCheck, objectWithoutProperties, and slicedToArray shapes are each
  matched twice — once as a declared function in the scanner, again as an
  inline expression inside the consuming rule.

So there are two distinct problems:

1. **One query surface where rules need two.** `collect_transpiler_helpers()`
   answers "which module-level bindings are helpers?" but not "is this
   expression an inlined helper?". Rules answer the second question privately.
2. **Matcher expression cost.** Hand-written Rust AST destructuring costs
   roughly 10x the lines of the JS it recognizes; 40–66% of matcher LOC is
   let-else/match-pyramid navigation rather than semantic checks. Every new
   helper, variant, or Babel version pays this again.

## Phase 1 — expression-site channel on `LocalHelperContext`

Mechanical consolidation, no new matching technology:

- Add an expression-site query to the helper context, reusing the existing
  body matchers on `FnExpr`/`ArrowExpr` nodes:

  ```rust
  /// Classify an expression as an inlined transpiler helper, if it is one.
  /// Covers IIFE forms like `((e) => e && e.__esModule ? e : {default: e})(x)`.
  pub fn classify_inline_helper_call(
      call: &CallExpr,
  ) -> Option<(TranspilerHelperKind, &Expr /* the wrapped argument */)>
  ```

- Migrate the six rules' private detection onto it. The rewrite halves of those
  rules are untouched; only their "is this the helper?" code moves or is
  deleted.
- esbuild-specific spread helpers currently detected inside `un_object_spread`
  move behind the same registry with their own kind variants, so "what counts
  as a helper" has a single home even when the shape is bundler-flavored.

Expected effect: ~1,000 lines consolidated, the four concrete duplications
removed, and — more importantly — a single choke point through which all
detection flows, which phase 2 requires.

Every migrated rule keeps its existing unit tests; inline-form regression tests
move to the shared matcher's test file rather than being re-asserted per rule.

## Phase 2 — pattern-as-JS matching engine (shadow mode)

Replace hand-written predicates with **patterns written as JavaScript source**,
matched by one generic engine:

- **Corpus**: vendored verbatim from what transpilers actually publish —
  `@babel/helpers` (across the versions we care about, including loose mode),
  `tslib` (1.x/2.x), `@swc/helpers`. One snippet per known variant of each
  helper, tagged with its `TranspilerHelperKind`. Adding a helper or a new
  Babel version is pasting its source, not writing Rust.
- **Engine**: structural equality modulo binding renaming (alpha-equivalence —
  the generalization of what `MatchContext` already does per-matcher), plus a
  small metavariable/ellipsis convention for the genuinely variable spots
  (e.g. `$ARG` for "any expression", statement ellipsis for tolerated extra
  guards). Estimated one-time cost: ~600–900 lines, with property that all
  future helpers are data.
- **Shared normalization**: both corpus and candidates are run through the same
  Stage-1 normalization slice (`UnBracketNotation`, `UnIndirectCall`,
  `UnminifyBooleans`, `RemoveVoid`, …) before comparison. This is the insight
  that changes the calculus relative to the original design notes: wakaru now
  *owns a canonicalizer*, so exact-tree-modulo-renaming covers most of what
  relaxed predicates were hand-tolerating. Residual variance (ternary vs
  if/else single-return) becomes either one matching-time canonicalization or
  one more corpus entry.
- **Performance**: candidates are pre-filtered by cheap signals (param count,
  statement count, presence of marker atoms like `__esModule`) before
  unification, so the engine runs on a handful of candidates per module —
  same scan cadence as today's lazily-built `LocalHelperContext`.

**Shadow mode**: the engine runs *alongside* the existing predicates across the
full fixture suites and the matrices in `scripts/repro/`, logging every
disagreement (helper found by one side only). No pipeline output changes. The
disagreement log is the decision artifact.

### What stays bespoke

Equality-plus-corpus will not reach:

- **asyncToGenerator / regenerator-adjacent shapes** — deep structural drift
  across versions; the existing relaxed marker-based matcher stays.
- **slicedToArray compositions** — minifiers inline the Babel sub-helpers
  (`arrayWithHoles`, `iterableToArrayLimit`, `nonIterableRest`) into each
  other. The corpus carries the sub-helpers individually (Babel ships them
  individually), which may decompose some of this naturally, but the composed
  inline-extraction path keeps its bespoke code until shadow data says
  otherwise.

The target for replacement is the simple-to-medium tier: the interops,
extends, classCallCheck, possibleConstructorReturn, assertThisInitialized,
toConsumableArray, objectSpread, objectWithoutProperties, defineProperty,
taggedTemplateLiteral. That tier alone is a large majority of matcher count
and roughly half the matcher LOC.

## Phase 3 — replace at parity, per helper

A predicate is deleted only when shadow mode shows, for that helper kind:

- zero missed detections across all fixtures and repro matrices, and
- zero new detections that are not verified true positives (a new true
  positive found by the corpus is a win, recorded as a fixture).

Helpers that never reach parity keep their hand-written matcher, documented as
the deliberate exception. Partial adoption is an acceptable end state — the
engine plus corpus must justify itself per helper, not all-or-nothing.

## Relation to prior design decisions

[helper-detection.md](../helper-detection.md) rejected a custom IR, CFG
hashing, and version auto-detection — this proposal is none of those. It is
adjacent to the rejected "general AST pattern DSL", so the difference is worth
stating: that rejection was made when matchers were few and small, and the
hard part of fingerprinting ("stable canonicalization") had no answer. Today
the matcher corpus is ~3,800 lines and growing with every supported
transpiler version, and Stage 1 *is* the canonicalizer. The fixed cost of one
unifier now amortizes; the marginal cost of the status quo no longer does.
Prior art in this exact domain: webcrack detects Babel helpers via template
matchers with capture variables; ast-grep demonstrates pattern-as-code
unification in Rust (not directly reusable — it operates on tree-sitter trees,
not mid-pipeline SWC ASTs).

## Risks

1. **Strictness misses unseen variants.** Equality is less forgiving than a
   relaxed predicate. Mitigations: corpus breadth is cheap (paste the
   variant); shadow mode finds gaps before anything is deleted; a diagnostic
   for "function that looks helper-shaped but matched nothing" can surface
   unknown variants from real bundles over time.
2. **Ellipsis/metavariable semantics creep.** The convention must stay small —
   if a pattern needs sophisticated control-flow tolerance, that helper
   belongs in the bespoke tier, not in a more powerful engine.
3. **Two systems during transition.** Shadow mode means running both. The
   per-helper parity gate keeps the transition window finite and the
   disagreement log makes the cutover auditable.

---

# UPDATE (validated) — from corpus matching to a skeleton-pattern + checker engine

The phases above were prototyped end-to-end in shadow mode against real bundles
(an external helper-bearing corpus) and the repro analysis. The data redirected
the effort. This section supersedes the corpus-replacement plan (Phase 3 above)
with the approach the experiments actually justify.

## What the experiments found

**Corpus / exact-canonical matching has a low ceiling.** Canonicalize-then-compare
(alpha-rename bound idents → normalize → string-compare against vendored
snippets) reached *exact parity with zero false positives* only on small,
structurally-stable helpers: `interopRequireDefault`, `classCallCheck`, and —
after fixing a free-vs-bound canonicalization artifact and adding an isolated
`UnConditionals` normalization pass — `objectSpread` and the *non-inlined*
OR-dispatcher forms of `slicedToArray`/`toConsumableArray`. It is **fundamentally
hopeless** on `interopRequireWildcard`, `objectWithoutProperties`, and any
**fully-inlined-sub-helper** body: those vary combinatorially per minifier and
are not canonical. Replaceable LOC via this route ≈ 22% of the matcher core, and
only the *cheap* matchers — it does not touch the giants (`slicedToArray` ~1.3k,
`asyncToGenerator` ~0.8k) that dominate the LOC. **Rejected as the primary lever.**

**The real footprint is bigger than the matcher core.** A full inventory found
**~209 detection functions / ~17.5k LOC** of helper/shape detection spread across
`transpiler_helper_utils.rs` *and* the rules (`un_es6_class`, `un_regenerator`,
`un_object_rest`, `un_async_await`, `un_object_spread`, …). ~55% of that, by line
count, is navigation/extraction boilerplate (let-else pyramids, match-arm
tree-walking, Box/Option unwrapping to *reach* the nodes worth checking).

**A skeleton-pattern + semantic-checker engine removes the boilerplate, not the
semantics — and that is where the lines are.** A prototype SWC-native engine
(holes bind to `(sym, SyntaxContext)` or subtrees; repeated holes enforce
same-binding identity) reimplemented two matchers as `pattern + checker`, proven
**equivalent to the originals with zero disagreements**:

- `classCallCheck`: 54 → 21 LOC (61% reduction)
- `interopRequireDefault`: 92 → 54 LOC (41%; dragged by an imperative
  if/return↔ternary desugar — see alternation, below)
- marginal cost per additional fixed-shape matcher: ~21–25 LOC

The binding-identity precision (`$obj` repeated must be the *same* binding) is
preserved through `SyntaxContext` — the precision a tree-sitter/ast-grep textual
matcher would lose, and the reason this is SWC-native rather than off-the-shelf.

## Scope criterion (what the engine is and isn't for)

- **IN (fixed-shape):** a helper recognizable as a fixed skeleton modulo
  binding-renaming plus a small residual semantic check. These compress ~45–60%.
  Candidates: `classCallCheck`, `interopRequireDefault`, the simple
  `interopRequireWildcard` forms, `possibleConstructorReturn`,
  `assertThisInitialized`, `defineProperty`, `taggedTemplateLiteral`, `extends`,
  `objectSpread`, the self-contained `toConsumableArray` form.
- **OUT (marker-based / signal-accumulation):** helpers matched by "has marker X
  *somewhere* in an unbounded body" (`Array.isArray`+`Symbol.iterator` for
  `slicedToArray`; promise/generator signals for `asyncToGenerator`/`__awaiter`;
  the `un_regenerator` state machines; the `un_es6_class` composition context).
  A skeleton does not fit these; they stay bespoke (checker-heavy). This is where
  most of the 17.5k lines live, so the realistic *whole-codebase* reduction is
  ~25–40%, not 55% — a strong cut on the fixed-shape subset, a wash on the giants.

## Engine design

- **Patterns** are authored as skeletons with `$NAME` holes over the SWC node
  kinds the in-scope matchers need (if/throw/return statements, unary `!`, paren,
  binary `instanceof`/`&&`, conditional `?:`, member incl. computed string-literal
  props, call, `new`, one-prop object literal, ident).
- **Holes** bind to a concrete `(sym, SyntaxContext)` (ident holes) or a subtree
  (expr holes). A repeated hole requires the same binding at every occurrence
  (alpha/binding identity, reusing `MatchContext` semantics).
- **Alternation primitive (required):** a pattern is a set of skeletons; match
  succeeds if any alternative matches. This absorbs structural variants
  (if/return vs ternary) declaratively, eliminating the imperative desugar that
  dragged `interopRequireDefault` down to 41%. Without it the engine
  under-delivers; with it the fixed-shape subset compresses toward the 60% case.
- **Checker:** a small per-helper closure over the bound holes verifies residual
  semantics the skeleton can't (e.g. `$I`/`$C` are the two params; `$E` is the
  global `TypeError`). The engine *preserves* the captured ctxt, so checkers may
  optionally be *stricter* than today's name-only global checks.
- Builds on existing primitives: `match_context.rs` (slots), `helper_matcher.rs`
  (binding identity), `expr_utils.rs` (structural compare) — ~80% of the
  foundation already exists.

## Migration discipline (per matcher)

1. Author the pattern(s) + checker as a `_v2` function alongside the original.
2. Equivalence test: over rule-test inputs + synthetic variants + negatives (and
   the external bundle corpus where convenient), assert `_v2` agrees with the
   production matcher on every input — **zero disagreements** required.
3. Only then route production detection through `_v2` and delete the original.
4. Full pipeline tests + snapshots must stay green (behavior-preserving), and the
   per-matcher LOC delta is recorded.

Partial adoption is the intended end state: migrate the fixed-shape subset, leave
the marker-based giants bespoke and documented as the deliberate exception.
