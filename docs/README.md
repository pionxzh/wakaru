# Docs Index

Start with [architecture.md](architecture.md) — what wakaru does, the
pipeline flow, and the design patterns every change touches. Everything else
is read-on-demand by task. Don't read the whole directory; use the map.

For rule ordering, stages, and enforced edges, the registry in
`crates/core/src/rules/pipeline.rs` is authoritative — the docs explain *why*,
the registry defines *what*.

## Read by task

| Task | Read |
|---|---|
| Any code change | [testing.md](testing.md) — test patterns, helpers, required verification before commit |
| Rule bugfix / snapshot regression | [debugging.md](debugging.md) — rule tracing, snapshot layers, fixture workflow |
| New rule, or moving a rule | [rule-dependency-inventory.md](rule-dependency-inventory.md) — ordering rationale, fragile edges, experiment log; [rewrite-assumptions.md](rewrite-assumptions.md) — level gating and named assumptions |
| Transpiler helper work | [helper-detection.md](helper-detection.md) — detection design and rejected alternatives |
| Cross-module / unpack behavior | [fact-system.md](fact-system.md) — the two-phase barrier and module facts |
| Correctness / semantics questions | [rewrite-assumptions.md](rewrite-assumptions.md); [test262-roundtrip.md](test262-roundtrip.md) — the semantic round-trip harness |
| Before proposing a redesign | [learnings/](learnings/) — approaches already built, measured, and reverted |
| Cutting a release | [releasing.md](releasing.md) |

## Data directories

- [test262-baselines/](test262-baselines/) — tracked Test262 baseline
  summaries (current totals cached in `scripts/correctness/test262-stats.json`)
- [proposals/](proposals/) — design proposals, deferred or in progress
- [learnings/](learnings/) — post-mortems of measured-and-reverted approaches
