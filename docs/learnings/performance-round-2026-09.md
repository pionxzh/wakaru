# Learning: what moved the needle in the September 2026 performance round, and what did not

**TL;DR — Five changes were kept: move untouched statements instead of
cloning them when a rule rebuilds a statement list, run a cheap local shape
check before building a whole-module `BindingUseIndex`, install mimalloc in
the CLI binary, dispatch modules largest-first in both unpack phases, and hash
internal binding tables with `FxHasher`. Measured together in one session
against the `main` they were based on, with alternating AB/BA pairs, they cut
end-to-end time on medium bundles by 27–29% and on a large esbuild bundle by
42% on an Apple M2 Max, with byte-identical output and 6–50 MiB more peak RSS.
Four ideas were tried and rejected: skipping the index for temporary-binding
proofs, an esbuild AST handoff (and its parallel-emit fallback), reordering the
`SimplifySequence2` side-effect check, and gating Phase 1 fact recovery on
IIFE presence. Do not re-measure these without a new angle. Measure with
alternating AB/BA runs, and not right after a long build: sequential batches
produced 2–3% "gains" that vanished when the order was alternated, and a
batch started minutes after two fat-LTO builds showed 8–50% run-to-run spread
that a short cooldown removed.**

## What worked

**1. Move, don't clone, when rebuilding statement lists.** `UnObjectRest`
rebuilt every enclosing statement list with `iter().cloned()` and
`stmt.clone()`, so a deeply nested function was deep-copied once per nesting
level even when nothing in it changed. Consuming the list with an owning
iterator and moving unchanged statements removed 8–9% of end-to-end time on
medium bundles by itself. `IntoIter::as_slice()` keeps the original suffix
available for look-ahead proofs, so no proof logic changed. Any rule that
rebuilds `Vec<Stmt>` or `Vec<ModuleItem>` with clones is a candidate for the
same treatment; a regression test can pin it by comparing `*const Function`
addresses before and after the rule.

**2. Prove "nothing to do" before building analysis.** `UnEsm` runs three
times per module and built a full `BindingUseIndex` plus two CommonJS evidence
collectors on every run, including on modules already converted to ESM. The
resolver-aware unresolved-name inventory already says whether `require`,
`exports`, or `module` appear; gating the CommonJS proofs on it, and putting
a `windows(2)` or single-declarator pre-check in front of the three pre-pass
helpers, removed about 38% of index builds. A second traversal that built a
legacy occurrence-count map inside every index was moved to its only consumer
(`BindingFacts`). Accumulated `UnEsm` time on the largest bundle dropped by
about 70%. The esbuild ownership graph's per-declaration reference and write
queries are independent and now run on the Rayon pool with the caller's SWC
`GLOBALS` installed on each worker; a test pins identical maps for one and
four workers.

**3. mimalloc in the CLI.** A native CPU sample showed the shared cost that
rule-level traces attribute to whichever rule happens to allocate: macOS
small-object `malloc`/`free`. Installing `mimalloc::MiMalloc` as the global
allocator in `crates/cli/src/main.rs` alone removed 26–30% of remaining time
on the larger bundles, at the cost of 11–14% more peak RSS. The allocator is
deliberately confined to the executable; the `wakaru` façade, core, and WASM
crates leave the choice to their callers. swc's own `swc_malloc` makes the
same choice on the platforms Wakaru ships to; its jemalloc and system-allocator
fallbacks exist for armv7 and musl, which Wakaru does not build.

**4. Dispatch the largest modules first.** Both unpack phases mapped modules
with `par_iter`, whose recursive range splitting can start the largest module
after most workers have gone idle, so one module becomes the critical path.
Sorting by source length and pulling through `par_bridge` (longest-processing-
time-first) lifted the Phase 2 parallel speedup on the large bundle from about
7x to 11.5x on 12 cores, a 20% wall-time cut, with results re-sorted to input
order so scheduling never reaches the output. It does nothing for a bundle
whose single largest module already dominates; only making that module's rules
cheaper helps there.

**5. `FxHasher` for internal tables.** A CPU sample after the allocator change
put SipHash and hashbrown lookups at about 15% of compute; two thirds of it
sat in `binding_uses`, `un_optional_chaining`, `smart_rename`, and
`rename_utils`, all keyed by `(Atom, SyntaxContext)`. Those keys already hash
to a precomputed word, so the keyed rounds bought nothing. Routing every
internal table through `crate::collections::{HashMap, HashSet}` took a further
9–10% off the medium bundles and 6% off the large one. Iteration order was
never observable (the default hasher is seeded per process), so the swap
changes cost, not results. Swapping only the hot files does not work: the
sets cross function boundaries everywhere, and the type mismatches force the
crate-wide alias anyway.

## What did not work

**Skipping the index for temporary-binding proofs.** Collecting uninitialized
binding IDs during the legacy count traversal and skipping the full use-site
index when only hoisted `var` temporaries exist cut index builds by another
31%. Accumulated index time barely moved and wall time did not move at all:
the skipped indexes were the cheap ones, and the expensive builds on complex
modules survived. Lesson: reduce the cost of the surviving expensive calls,
not the call count. The equivalence oracle (19 shapes covering shadowing,
redeclaration, switch, for-in/of, class bodies, destructuring defaults) was
useful and is worth reviving if the expensive path is ever attacked.

**esbuild AST handoff.** The idea was to hand recovered modules to the rule
pipeline as ASTs instead of printing and reparsing. Two facts killed it before
implementation: the intermediate print/reparse measured far smaller than the
stage-level intake timing suggested, and esbuild assembles CommonJS cache
wrappers, ESM init guards, export storage, and redirects partly as strings, so
a structured handoff needs correct resolver identity, relative span ordering
for TDZ proofs, raw-output materialization, and source-map-mode behavior. The
webpack/Metro sidecar is the right driver boundary but does not cover those.
A bounded parallel-emit fallback did not compile: `Lrc<SourceMap>` is not
`Sync` under the workspace's SWC feature set, and changing that would widen
the change past the saving.

**Reordering the `SimplifySequence2` side-effect check.** Moving the existing
side-effect check ahead of the observable-read proofs passed its tests and
measured within noise on seven alternating pairs. Reverted.

**Gating Phase 1 fact recovery on IIFE presence.** Phase 1 clones every
module and re-runs UnIife, its arrow preparation, the UnCurlyBraces..UnEsm
range, SmartRename, and UnExportRename before collecting facts. A read-only
"does the module contain a call whose callee is a function expression" scan
skipped that work for most modules with byte-identical output on every
benchmark input, and it was still withdrawn. The predicate covers what UnIife
can expose; the skipped work also includes inlining and export renames that
change which local an `ExportFact` names, and helper-export classification
reads that local. A module exporting a helper through an alias
(`var h = p; export { h as rest }`) has no IIFE, yet only the recovered clone
classifies it as a helper module. A gate whose predicate does not cover every
pass it skips cannot be hardened case by case; the honest options are to gate
each pass on its own trigger or to leave the recovery alone.

## Measurement rules that mattered

- Alternate AB/BA and compare within one method. Serial hyperfine batches
  drifted with machine conditions and manufactured a 2.5% gain and a 3%
  regression that both disappeared under alternation.
- Keep outliers in the data and report medians alongside means.
- Treat accumulated per-rule durations as diagnostic sums across parallel
  workers, never as wall-clock stage time.
- Gate every candidate on identical output filenames and bytes across every
  benchmark input before looking at timing.
- Profile after each retained change; the allocator only became visible once
  the rule-level hot spots were gone.

## Remaining leads

- Output writing: after the changes above, roughly a third of busy CPU
  samples on the large bundle were threads blocked in `open`/`close`/`write`
  while creating thousands of output files, about half a second of wall time
  that runs after Phase 2 instead of overlapping it. Writing each module as it
  finishes was built as provisional per-file output staging and measured
  slower on the large and medium bundles: it moved the filesystem work
  earlier instead of removing it. Reopen this lead only with a design that
  removes filesystem work. Publishing the whole output directory at once is
  untested.
- Few-module bundles: one module can hold 60% of Phase 2 CPU, and `UnEsm`,
  `UnObjectRest`, `UnComputedProperties`, and `UnObjectSpread` each spend tens
  of milliseconds on it. Profile that module alone for superlinear behavior
  before touching the rules.
- esbuild intake: `detect_esbuild` is still single-threaded and about a quarter
  of the large bundle's wall time; scope-hoisted extraction is per-group and
  independent, emission is blocked by `Lrc<SourceMap>` not being `Sync`.
- Accumulated rule sums on the large bundle are now flat (the top rule is
  about 7%); further per-rule work has diminishing returns compared with the
  cross-cutting items above.

## Allocation follow-up, September 12

Object-rest's module-level recovery still deep-cloned every emitted statement
into a second history vector for its backward proofs, even after the output
rebuild itself became move-based. Borrowing the rebuilt output instead cut
full-core allocation request bytes by 1.7–8.0% across three full-unpack
inputs. Object-spread recovery cloned the argument trees of nested helper
calls once per enclosing call; moving them removes that repeated copying but
changes allocation on the same inputs by at most 0.03%. Timing moved between
flat and about 3% better. Neither change is a throughput claim. Extra
parallel esbuild factory metadata analysis was also tried and dropped because
end-to-end timing showed no gain.

`cargo run -p wakaru-core --example allocation_probe -- <rule|pipeline|unpack> input.js output`
counts allocator requests around one rule, the rule pipeline, or a one-worker
unpack, using the System allocator in the dev profile. Counts repeat exactly
between runs, so they are a deterministic complement to the timing rules
above. They measure allocation traffic, not peak memory or latency.
