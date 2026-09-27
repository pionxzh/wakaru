# SWC Minifier Reproduction Matrix

This harness checks how focused SWC minifier options lower small JavaScript
snippets, then runs Wakaru on the lowered output. It is for investigation, not a
committed snapshot source.

Use the local sibling `../swc` repository as a reference for minifier source and
test shapes. The matrix owns the Wakaru-facing expectations because SWC's tests
assert minified output, not decompiler output.

```powershell
node scripts/repro/swc-minifier-matrix/matrix.mjs
```

Add `--details` to print full original, lowered, and recovered code for missed
cases. Add `--level minimal`, `--level standard`, or `--level aggressive` to run
Wakaru with a specific rewrite level.

Rows marked `no` or `info-miss` are investigation findings and do not make the
script exit non-zero. The script exits non-zero only when SWC or Wakaru cannot
run.

The matrix installs the `@swc/core` release pinned in `lib/runner.mjs`
(`SWC_CORE_VERSION`) under `target/repro-tools/`.
Set `WAKARU` to test a specific binary:

```powershell
$env:WAKARU = "$PWD\target\debug\wakaru.exe"
node scripts/repro/swc-minifier-matrix/matrix.mjs --details
```

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact minifier
shape is preserved while duplicate tool outputs are collapsed.

The `babel-<version>-class-*` rows lower ES classes with Babel `preset-env`
(IE 11) first, then minify Babel's output with each SWC profile. SWC inlines
the single-use `_createClass`, `_defineProperties`, and `_classCallCheck`
helpers into the class, which is the shape these rows track. The helper
bodies (for example, whether `_defineProperties` routes keys through
`_toPropertyKey`) come from `@babel/helpers` and the class plugin, not from
core. The shared tool installer resolves each pinned Babel release's
dependencies as of its publish date, so each row lowers with that release's
own helpers and plugins.

The class rows skip the `inline-iife` profile. It turns every other compress
default off, so constant folding and dead-code removal never finish the
`_createClass` expansion (`if (protoProps) _defineProperties(...)` stays). SWC's
default compress options fold that half-inlined shape away (the `all` profile).

Some rows are informational (`informational: true`). Constant folding and full
inlining can erase the original structure, so those rows document what Wakaru
currently emits rather than implying a reversible source recovery is expected.
They report `info-yes` or `info-miss` and stay out of the recovery rate either
way; `collect-stats.mjs` records their count as `info`. An execution
divergence on an informational row still reports `no`.

## Promoting Findings

When the matrix finds a useful missed shape, minimize the lowered code and add a
focused Rust unit test. Do not rely on matrix output or pipeline snapshots alone.

- Sequence recovery belongs in `crates/core/tests/simplify_sequence_rule.rs`.
- `!0` / `!1` recovery belongs in `crates/core/tests/unminify_booleans_rule.rs`.
- `!!x` recovery belongs in `crates/core/tests/un_double_negation_rule.rs`.
- Inline and IIFE findings belong in `smart_inline_rule.rs` or `un_iife_rule.rs`.
- Mangle/name recovery findings belong in `smart_rename_rule.rs`, unless the
  finding is helper body detection, in which case use the specific helper test
  or `pipeline_helpers_rule.rs`.

Record the producer in the test comment, for example:

```rust
// Produced by @swc/core minify with compress.sequences=true, mangle=false.
```
