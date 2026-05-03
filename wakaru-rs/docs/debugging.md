# Debugging

This document collects workflow notes for investigating rule and snapshot
regressions in `wakaru-rs`.

## Rule Trace

Use the rule trace CLI before manually bisecting with `apply_rules_between`.
It runs the normal single-file rule pipeline and prints the initial source
once, followed by a git-style unified diff for each rule that changes the
rendered code. Rules that ran but left the output unchanged (with `--all`)
show up as a single `=== RuleName (unchanged) ===` header.

```bash
cargo run -- debug trace path/to/module.js
```

Useful options:

```bash
# Include rules that ran but did not change rendered output
cargo run -- debug trace path/to/module.js --all

# Trace only a range of rules
cargo run -- debug trace path/to/module.js --from RemoveVoid --until UnEsm
```

Rule names are the names returned by `rule_names()`, for example
`RemoveVoid`, `UnIife`, `SmartInline`, or `UnReturn`.

`debug trace` is intentionally single-file only. Bundle decompile uses the
two-phase fact-system pipeline, so tracing a full bundle would be misleading.
For bundle regressions, trace the extracted raw module or reduce the issue to a
single-file reproduction.

## Snapshot Layers

Webpack4 has two snapshot layers:

- `webpack4_unpack__*.snap` — final decompiled output, after rules.
- `webpack4_unpack_raw__*.snap` — raw module output after webpack
  normalization, before rules.

When a snapshot changes unexpectedly, compare the raw and final snapshots for
the same module. If the raw snapshot is unchanged but the final snapshot moved,
the cause is in the rule pipeline. If raw output changed too, inspect the
unpacker or webpack normalization first.

## Common Symptoms

- **Unexpected variable names:** Check for a missing `unresolved_mark` guard or
  matching by `sym` instead of `(sym, SyntaxContext)`.
- **Too many snapshots changed:** An early pipeline rule is cascading. Use
  `debug trace` on a representative module and check early rules like
  `SimplifySequence`, `FlipComparisons`, and `RemoveVoid`.
- **Rule not firing:** Check the raw snapshot. Earlier passes may have changed
  the AST shape before your rule runs.
- **`cargo test` hangs:** Likely infinite recursion. Run with
  `RUST_BACKTRACE=1 cargo test -- --nocapture`.

## Fixture Repo

A private fixture repo at `../wakaru-fixtures/` contains bundled demo apps and
real-world bundles for cross-bundler regression testing. After significant rule
changes, run `./run.sh` there and check `git diff` for regressions.

Always rebuild the release binary before running fixtures. `run.sh` invokes
`target/release/wakaru` by default; it does not rebuild it for you.

```bash
cargo build --release && (cd ../../wakaru-fixtures && ./run.sh)
```

Before trusting a fixture diff, sanity-check the binary timestamp against your
most recent code change.
