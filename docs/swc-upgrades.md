# Upgrading swc

Wakaru pins `swc_core`. A bump is worth doing mainly for parser correctness.
When a swc bug breaks Wakaru output, fix it on the Wakaru side first rather
than waiting for a release.

## Evaluating a bump

Read the changes in the crates Wakaru uses, not the whole changelog. Diff
these directories between the two `swc_core@vX` tags of a swc checkout:

```text
crates/swc_ecma_ast/src
crates/swc_ecma_parser/src
crates/swc_ecma_codegen/src
crates/swc_ecma_utils/src
crates/swc_ecma_visit/src
crates/swc_common/src
crates/swc_ecma_transforms_base/src
```

Minifier entries in the changelog do not affect Wakaru. Parser, codegen, and
utils entries do. A `swc_core` major can come entirely from minifier or
other crates while every crate Wakaru uses moves by a patch, so check the
per-crate versions in `Cargo.lock` before assuming API work.

A parser fix that changes what reaches the rules also moves Test262
baselines; [test262-roundtrip.md](test262-roundtrip.md#baselines) has the
accept flow.

## Upstream workarounds

When upstream ships a fix for something Wakaru worked around, delete the
workaround and keep its tests. The tests stay as the tripwire if the bug comes
back; never delete them with the workaround. Check for such workarounds on
every bump.

## Checklist

Besides the [required verification](testing.md#required-verification-before-commit):

1. Run the Test262 matrix. Review and accept each `.json.new`, refresh
   `test262-stats.json`, and update the total cited in `README.md` and the
   docs-site Correctness page by hand. Drop known-blocker entries whose
   reason no longer matches any result.
2. Run the private fixture suite (`../wakaru-fixtures/run.sh --check`) when
   the sibling checkout exists.
3. If the sibling checkout `../wakaru-private-artificial` exists, run the
   checks its `README.md` lists.

## Filing a swc issue

- The bug form requires a repro link from an allowed domain, such as
  `play.swc.rs` or a gist. Playground links carry `code` and `config` as
  base64 of the gzipped text; open a generated link in a browser before
  citing it. Put a Rust repro under Additional context.
- Fixer and codegen bugs on JSX reproduce in `@swc/core` with
  `jsc.transform.react.runtime: "preserve"`.
- Title the issue `es/<area>: <behavior>`, for example `es/fixer: …`.
- Before calling output invalid, check Babel 8 too. npm `latest` for
  `@babel/parser` is still 7.x, so pin `@babel/parser@8`. Babel 7,
  TypeScript, esbuild, and oxc accept some syntax that Babel 8 rejects per
  the spec, such as a sequence expression in a JSX container.
