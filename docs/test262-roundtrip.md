# Test262 Round-Trip

`scripts/correctness/test262-roundtrip.mjs` checks semantic preservation by
running a Test262 file in Node's `vm`, transforming it, decompiling it with
wakaru, and running the decompiled result through the same Test262 harness.
Module tests are different: `--preset modules` follows static relative
imports/re-exports, writes a temporary ESM graph, and runs that graph in Node.

The runner is intentionally feature-scoped. Prefer `--preset` or focused
`--path` values over running the whole Test262 repository.

Test262 frontmatter is parsed strictly. Unknown flags, unknown negative phases,
conflicting strictness flags, missing frontmatter, and malformed relevant fields
or referenced harness files are reported as `harness-configuration` failures
rather than silently defaulted, skipped, or aborting the corpus. Ordinary
scripts receive sloppy and strict variants; `onlyStrict`, `noStrict`, `module`,
and `raw` select the corresponding Test262 variant.

## Corpus setup

The default Test262 corpus is a managed, shallow checkout pinned by
`scripts/correctness/test262-upstreams.json`. It is stored under the ignored
`target/correctness-tools/test262/vendor/` directory rather than as a git
submodule.

```powershell
node scripts\correctness\test262-corpus.mjs setup
node scripts\correctness\test262-corpus.mjs status
node scripts\correctness\test262-corpus.mjs setup --offline
node scripts\correctness\test262-metadata-audit.mjs
```

Setup refuses to modify a dirty checkout. `--force` explicitly replaces dirty
or mismatched fixture state. Updating the tracked revision is separate and does
not regenerate or classify baselines:

```powershell
node scripts\correctness\test262-corpus.mjs update --revision <full-commit-sha>
```

`--test262 <dir>` remains available for focused work with another checkout;
reports identify such a non-git fixture as `unmanaged`.

Run the metadata audit after changing the pin. It parses every non-fixture
JavaScript test, not only the paths selected by the baseline matrix, and fails
on new flags, negative phases, or malformed frontmatter.

## Commands

```powershell
node scripts\correctness\test262-roundtrip.mjs --limit 500
node scripts\correctness\test262-roundtrip.mjs --limit all --json target\test262-default.json
node scripts\correctness\test262-roundtrip.mjs --limit all --summary target\test262-default.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline babel-env-terser --limit 100 --summary target\test262-classes-babel.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline swc-minify --limit 100 --summary target\test262-classes-swc.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --pipeline esbuild-minify --limit 100 --summary target\test262-classes-esbuild.md
node scripts\correctness\test262-roundtrip.mjs --preset classes --limit all --json target\test262-classes.json
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline swc-minify --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-swc.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline esbuild-minify --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-esbuild.md
node scripts\correctness\test262-roundtrip.mjs --preset modules --pipeline babel-env-terser --limit all --case-timeout-ms 2000 --summary target\test262-modules-graph-babel.md
node scripts\correctness\compare-test262-reports.mjs target\before.json target\after.json --details
node scripts\correctness\test262-roundtrip.mjs --rerun-from target\test262-default.json --rerun-status failed --json target\test262-default-rerun.json
```

Defaults:

- `--pipeline terser-light`
- legacy equivalent: `--transform terser --terser-profile light`
- `--terser-profile light`
- `--level minimal`
- `--known-blockers scripts/correctness/test262-known-blockers.json`
- default paths: coalesce, optional chaining, object expressions, array
  expressions, `for-of`, and `let`

Use `--summary <file>` when you want a stable Markdown report suitable for
reviewing baseline movement in git diffs. It records options, totals,
reason-count buckets, and current Wakaru failures without timestamps.

Canonical baselines are deterministic JSON files keyed by the pinned Test262
revision, harness version, Node major version, producer version/configuration,
Wakaru level, and selected preset. They store reviewed non-passing outcomes by path, variant,
typed reason, and a stable fingerprint that includes emitted-code hashes while
excluding machine-specific stack frames. Complete baseline runs fail on new or
changed outcomes, disappeared outcomes (including unexpected passes), or total
movement. A failed comparison writes the complete actual baseline beside the
reviewed file as `<baseline>.json.new`, analogous to an insta `.snap.new` file.
The reviewed JSON remains unchanged until that candidate is explicitly
accepted. The timeout budget is also part of the reviewed identity; the parallel
matrix defaults to 15 seconds to avoid load-dependent process-startup timeouts.
That budget is applied consistently to harness and test execution as well as
producer/decompiler subprocesses. The canonical matrix runs one slice job at a
time because each slice already parallelizes Wakaru invocations internally;
cross-slice parallelism made producer startup and VM timeouts load-dependent.
Filtered and limited runs cannot read or update a complete baseline.

When `--json` or `--summary` is provided, the runner updates that file after
each processed test. Interrupted runs leave `complete: false` in the report, so
the last saved result is still inspectable.

## Pipelines

`--pipeline` selects the producer that creates the code Wakaru decompiles:

- `none`: run the original Test262 source through Wakaru.
- `terser-light`: current default; Terser prints/minifies without compression or
  mangling.
- `terser-full`: Terser with compression and top-level mangling.
- `babel-env-terser`: Babel `preset-env` targeting IE 11, then `terser-light`.
- `swc-minify`: SWC minifier with compression and mangling disabled.
- `esbuild-minify`: esbuild transform with syntax/whitespace minification and
  stable identifiers.

Babel is an input producer, not the correctness oracle. The Test262 harness
remains the oracle: the original source, produced source, and Wakaru output must
all pass the same test.

## Execution semantics

The runner treats Test262 metadata as executable expectations:

- positive scripts must succeed in every selected sloppy/strict variant;
- `negative.phase: parse` and `early` tests run in a parser-boundary lane and
  must fail with the declared error type; they are not sent through a producer
  or Wakaru because invalid programs are not round-trip inputs;
- runtime and module-resolution negatives must preserve both phase and error
  type in the original, produced, and decompiled programs;
- `async` tests wait for `$DONE` (including `doneprintHandle.js` output), bound
  by the case timeout;
- `raw` tests do not receive the default `assert.js` and `sta.js` harness;
- script realms provide fresh `$262.createRealm()` contexts,
  `$262.evalScript()`, and ArrayBuffer detachment. Agent coordination,
  `IsHTMLDDA`, non-deterministic tests, and unavailable runtime features are
  classified from metadata as explicit unsupported capabilities.

## Module Graphs

`--preset modules` runs Test262 files with `flags: [module]` as module graphs
instead of single script files. The runner:

- parses static relative `import` and `export ... from` specifiers;
- recursively collects the entry module and local module dependencies;
- transforms every module with the selected producer in ESM mode;
- decompiles every transformed module independently with Wakaru;
- executes the original, transformed, and decompiled graphs through a temporary
  Node ESM package.

This is still not bundle testing. No packer is involved, and Wakaru is not run
with `--unpack`. The current purpose is to catch correctness bugs caused by ESM
semantics across files: live bindings, namespace objects, cycles, TDZ, export
aliases, and top-level await.

## Timeouts and Reruns

`--case-timeout-ms <n>` bounds each runnable test case. The default is 5000 ms.
Timeouts are recorded as `rejected` with reason `case-timeout`, so they are
visible in JSON and Markdown reports without losing the whole run.

Use `--rerun-from <json>` to rerun paths selected from a previous report. By
default it reruns `failed` results. Add one or more `--rerun-status` values to
include `rejected` or `unsupported` results too:

```powershell
node scripts\correctness\test262-roundtrip.mjs --rerun-from target\before.json --rerun-status failed --rerun-status rejected --json target\rerun.json
```

## Status Buckets

- `passed`: the typed metadata expectation is preserved. For positive and
  runtime/resolution-negative round trips this covers original, transformed,
  and decompiled code; parse/early negatives pass in the parser-boundary lane.
- `unsupported`: the local Node/vm/SWC parser setup cannot run this input.
- `rejected`: the transform/minifier or known SWC print fidelity issue blocks
  the case before it can be treated as a Wakaru semantic failure.
- `failed`: a current Wakaru correctness candidate.

Known non-Wakaru reasons currently classified:

- `node-parse-baseline`
- `node-vm-baseline`
- `node-module-baseline`
- `module-graph-baseline`
- `runtime-capability` with a typed host/feature reason
- `transform-reject`
- `transform-runtime`
- `sloppy-only-strict-ident`
- `swc-parse-async-ident`
- `swc-parse-await-class-name`
- `swc-parse-deep-iife-stack-overflow`
- `swc-parse-static-init-await`
- `swc-parse-static-async-constructor-method`
- `swc-parse-yield-arrow-parameter`
- `swc-parse-yield-function-name`
- `swc-parse-yield-ident`
- `swc-parse-yield-label`
- `swc-array-binding-elision`
- `swc-print-class-extends-arrow-parens`
- `swc-print-export-default-function-expression`
- `swc-print-new-arrow-parens`
- `swc-print-static-constructor-method`

Most known non-Wakaru classifications live in
`scripts/correctness/test262-known-blockers.json`. Keep entries narrow: match the
status, phase, path shape, error text, and decompiled output shape when possible.
Do not add a manifest entry for a real Wakaru semantic failure; fix the rule or
record it as a `failed` baseline instead.

## Baselines

Tracked baseline summaries live in `docs/test262-baselines/`. Normal summaries
are grouped by producer pipeline, and each producer runs the same slice set.
Regenerate the full normal matrix with:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs
```

Baseline paths encode two separate dimensions:

- the Test262 slice (`default`, `classes`, `modules`, and so on);
- the producer pipeline that transforms source before Wakaru runs
  (`terser-light`, `swc-minify`, `esbuild-minify`, and so on).

For example, `docs/test262-baselines/terser-light/default.md` means "default
Test262 slice through `terser-light`", not raw Test262 source. Use
`--pipeline none` or `--transform none` for a no-producer run. `terser-light`
uses Terser as a parser/printer with no compression or mangling.

Use `--producer` and `--slice` to refresh a subset:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs --producer swc-minify --slice operators
```

When comparison reports intentional movement, review the Markdown summary and
the adjacent `.json.new` candidate, then promote it without rerunning Test262:

```powershell
node scripts\correctness\test262-baseline-matrix.mjs --producer swc-minify --slice operators --accept
node scripts\correctness\test262-collect-stats.mjs --producer swc-minify --slice operators
```

Candidates record the exact reviewed baseline they were compared against.
Acceptance refuses stale candidates if that baseline has since changed; rerun
the comparison to produce a fresh candidate.

The clean comparison removes stale candidates. `--update` remains available
for deliberate baseline-identity migrations, but bypasses the candidate review
gate and should not be the initial command for ordinary Wakaru changes. CI
still performs an independent clean comparison of the accepted baseline.
Normal comparisons validate the stored identity before discovering or running
tests, and leave summaries and candidates untouched when the Test262 revision,
Node major, harness, producer, Wakaru options, or selection does not match.

Add `--missing` to skip summaries that already exist and have `complete: true`.
The matrix runner builds `wakaru-cli` once before running jobs unless `WAKARU`
is already set.
The lower-level roundtrip runner does not fall back to `cargo run`; build the
CLI first or set `WAKARU` before calling it directly.

The `Test262 Correctness` workflow runs the tooling tests and corpus-wide
metadata audit first, then compares all canonical baselines in one isolated CI
job per producer on Node 24, matching the tracked baseline identity. It is
path-gated for correctness-related changes, can be run manually, and also runs
weekly to catch runtime or infrastructure drift.

The roundtrip runner batches Wakaru invocations and runs them concurrently
(bounded by `cpus - 2`). The matrix runs one producer/slice job at a time because
each slice already parallelizes decompilation internally and cross-slice load
made timeouts nondeterministic.

`swc-minify` and `esbuild-minify` are standalone producer pipelines; they are
not followed by Terser.

Pinned producer packages are installed in separate subdirectories under the
tool root. Keeping Terser, Babel, SWC, and esbuild isolated prevents npm from
pruning and reinstalling one producer while another matrix job starts.

Module graph baselines live under `docs/test262-baselines/module-graph/`:
these add no-transform and Babel producer coverage to the canonical recursive
modules slice. SWC and esbuild module graphs are already covered by their
normal `modules` slice, so they are not duplicated here. In this directory,
the file name is the producer pipeline. The baseline matrix includes these
jobs; select only them with `--slice module-graph`.

```powershell
node scripts\correctness\test262-baseline-matrix.mjs --slice module-graph
node scripts\correctness\test262-baseline-matrix.mjs --slice module-graph --update
```

The recorded totals are **not** kept inline here — they live in two places
that stay current:

- `scripts/correctness/test262-stats.json` — cached totals per producer and
  slice (update with `test262-collect-stats.mjs`, verify with `--check`);
- [test262-baselines/](test262-baselines/) — the full deterministic Markdown
  summaries per producer/slice, reviewed as git diffs when baselines move.

Treat baseline movement as follows:

- `failed` going down means correctness improved or a non-Wakaru blocker was
  classified.
- `failed` going up needs investigation.
- `unsupported`/`rejected` moving is acceptable only when the reason is
  understood and documented in the JSON/report.

## Stats

`scripts/correctness/test262-stats.json` caches the current baseline totals so
other sessions can read them without regenerating all summaries. Update after
baseline changes:

```powershell
node scripts\correctness\test262-collect-stats.mjs                               # update all
node scripts\correctness\test262-collect-stats.mjs --producer swc-minify         # one producer
node scripts\correctness\test262-collect-stats.mjs --slice classes --slice scope  # specific slices
node scripts\correctness\test262-collect-stats.mjs --check                        # verify freshness
```

When `--producer` or `--slice` is given, only matching entries are re-collected;
the rest are kept from the existing stats file.
