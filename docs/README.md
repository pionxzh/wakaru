# Docs Index

Start with [architecture.md](architecture.md) — what wakaru does, the
pipeline flow, and the design patterns every change touches. Everything else
is read-on-demand by task. Don't read the whole directory; use the map.
The architecture overview keeps shared invariants; update format-specific
behavior in the relevant topic document instead of expanding the overview.

For rule ordering, stages, and enforced edges, the registry in
`crates/core/src/rules/pipeline.rs` is authoritative — the docs explain *why*,
the registry defines *what*.

## Read by task

| Task | Read |
|---|---|
| Any code change | [testing.md](testing.md) — test patterns, helpers, required verification before commit |
| Finding source entry points | [code-map.md](code-map.md) — file lookup by subsystem |
| PR / branch review, handoff, or resuming research | [reviewing.md](reviewing.md) — scope, evidence, ownership transfer, and research resumption; [testing.md](testing.md#sharing-verification-results) — reuse and invalidation of test evidence |
| Rule bugfix / snapshot regression | [debugging.md](debugging.md) — rule tracing, snapshot layers, fixture workflow |
| New rule, or moving a rule | [rule-dependency-inventory.md](rule-dependency-inventory.md) — ordering rationale, fragile edges, experiment log; [rewrite-assumptions.md](rewrite-assumptions.md) — level gating and named assumptions |
| Transpiler helper work | [helper-detection.md](helper-detection.md) — detection design and rejected alternatives |
| Detection, unpacking, or scope-hoisted splitting | [unpacking.md](unpacking.md) — format shapes, factory normalization, raw/multi-input behavior, and fallback boundaries |
| Cross-module recovery | [fact-system.md](fact-system.md) — the two-phase barrier, module facts, and same-module proof boundaries |
| Bun single-file executables | [bun-standalone.md](bun-standalone.md) — binary container extraction, CLI behavior, safety, and current limits |
| Public Rust API | [public-api.md](public-api.md) — design decisions and boundaries; rustdoc (`cargo doc -p wakaru`) is the behavioral contract |
| Vue SFC recovery (`--vue-sfc`) | [vue-decompile.md](vue-decompile.md) — the recovery path and CLI behavior; [vue-sfc-recovery-status.md](vue-sfc-recovery-status.md) — experimental status and known gaps |
| Correctness / semantics questions | [rewrite-assumptions.md](rewrite-assumptions.md); [test262-roundtrip.md](test262-roundtrip.md) — the semantic round-trip harness |
| Before proposing a redesign | [learnings/](learnings/) — approaches already built, measured, and reverted |
| CLI flag or output changes | [cli.md](cli.md) — the CLI reference and source of truth for behavior detail; [../skills/wakaru/SKILL.md](../skills/wakaru/SKILL.md) — the agent skill (carries only what changes the commands an agent runs or how it reads output); [../docs-site/content/docs/reference/cli.mdx](../docs-site/content/docs/reference/cli.mdx) — the user-facing docs page. Keep all three in sync in the same commit |
| Agent / tool integration | [../skills/wakaru/SKILL.md](../skills/wakaru/SKILL.md) — the CLI-based agent surface |
| Cutting a release | [releasing.md](releasing.md) |

## Data directories

- [test262-baselines/](test262-baselines/) — tracked canonical Test262 JSON
  baselines and Markdown summaries (current totals cached in
  `scripts/correctness/test262-stats.json`)
- [proposals/](proposals/) — reviewed proposals for deferred or in-progress work
- [learnings/](learnings/) — post-mortems of measured-and-reverted approaches
