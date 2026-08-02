# Import-cycle premerge (driver-level SCC concatenation)

**Status: removed without ever being enabled.**

## What was built

Commit `24832f36` ("fix(unpack): preserve split module cycles", 2026-06-09)
introduced import-cycle handling for multi-module unpack in one piece:

1. A diagnostics pass (`collect_import_cycle_warnings`) that finds local
   import SCCs across emitted modules and reports them as
   `UnpackWarningKind::ImportCycle` warnings. **This shipped and is still
   live.**
2. A driver-level "premerge" (`merge_import_cycles` in
   `driver/unpack_cycles.rs`, ~700 lines with its `hoist_late_runtime_helpers`
   cleanup) that concatenated each SCC into its representative module before
   Phase 1: internal imports dropped, consumer imports retargeted, external
   imports deduplicated, plus safety preflights (duplicate-declaration checks,
   a 32-member size cap, a fast path for large components).

The merge path was **born disabled**: its gate
(`should_premerge_import_cycles`) returned `false` from the same commit that
added it, as did the raw-mode twin (`should_merge_raw_import_cycles`). No
commit ever enabled either gate. The machinery survived ~2 months as a
"hook for a future static validator" reachable only from its own tests, and
was deleted in the cleanup that added this note.

## Why it was never enabled

From the gate comment written at birth, plus the raw-mode twin's comment:

- **Native ESM cycles are often valid.** A local import SCC in recovered
  output is not by itself evidence of a broken split; merging on that signal
  alone rewrites correct output.
- **Concatenating SCCs reduces split fidelity.** The whole point of unpacking
  is recovering module boundaries; a cycle-triggered merge undoes them, and in
  raw mode it would do so before the user ever sees the extracted modules.
- **Merging hides import-synthesis bugs.** A spurious cycle is usually a
  symptom of an unpacker emitting a wrong import edge. Surfacing it as a
  diagnostic points at the bug; merging silently absorbs it.

## What replaced it

Cycle merging is legitimate exactly where the cycle is an artifact of
wakaru's *own* clustering rather than of the original bundle: the scope-hoist
splitter merges cyclic **synthesized** clusters before emission
(`d48a27d0`, `unpacker/scope_hoist.rs`), preserving the original single-file
initialization order. `--unpack=inspect` renders the finer cyclic clusters
without merging for static inspection.

The driver keeps only the diagnostics pass. Detector outputs whose recovered
edges are not source-level ESM edges (Closure ModuleManager fragments,
scope-hoist splits, nested scope splits) opt out of the warnings via
`UnpackResult::without_cycle_warnings` / the `report_import_cycle_warnings`
flag so users are not warned about cycles that are expected there.

## If you are tempted to rebuild it

A future repair that merges modules at the driver level needs a *proof* that
the specific cycle is invalid (e.g. a static TDZ/initialization-order
validator showing the split output cannot execute), not the mere existence of
an SCC. The deleted implementation — including its duplicate-declaration
preflights and consumer retargeting — is recoverable from git history at
`24832f36..8caff037` (`crates/core/src/driver/unpack_cycles.rs`).
