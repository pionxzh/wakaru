# Module-id extension policy: emitted filenames must not lie about content

Status: naming policy implemented (webpack unpackers, `e26f1194`); the
content-over-name recovery ladder below remains deferred work. The
heuristic/scope-hoist implementation note was verified unnecessary — those
paths already append `.js`. Origin: module-boundary eval item 5
("JavaScript retained under a non-JS extension" validator blind spot),
re-scoped to the naming layer after review discussion on 2026-08-09.

## Problem

Webpack `namedModules`-style bundles use original source paths as string
module ids (`"./src/style/index.less"`). The module body in the bundle is
the *loader's JavaScript output*, but the webpack4/5 unpackers derive the
output filename via `sanitize_relative_path` only (strip `./`, `../`,
backslashes) with no extension handling, so the tree emits `src/style/
index.less` containing JavaScript.

This is not merely cosmetic and not a validator problem:

- Node ESM picks module format by extension, so `import "./index.less"`
  fails with `ERR_UNKNOWN_FILE_EXTENSION` regardless of content. The
  emitted tree cannot load as ESM, which is the output contract `debug
  validate` checks.
- The filename claims the file is a stylesheet; the stylesheet does not
  exist in the bundle (only the compiled JS does).

The webpack string-id path is the outlier: the browserify, AMD, closure,
esbuild, and systemjs unpackers already append `.js` to non-js-like names.

## Policy

One rule, allowlist-shaped ("keep what we recognize, suffix everything
else") — never an asset-extension denylist:

```
final extension ∈ { js, mjs, cjs, jsx, ts, tsx, mts, cts } → keep name as-is
anything else (unknown extension, or no extension)          → append .js
```

`./src/index.ts` stays `src/index.ts` (JS is valid TS/JSX content, and
churning `.ts` outputs would invalidate many references for no gain).
`./src/style/index.less` becomes `src/style/index.less.js` — the
derived-artifact idiom (`app.js.map`, vue-tsc's `App.vue.d.ts`): the
artifact appends *its own format* to the *full source name*.

Rejected alternatives:

- Replace the extension (`index.less` → `index.js`): systematically
  collides with the ubiquitous `Button.less` + `Button.js` sibling layout,
  loses provenance, and dedup renames make results unpredictable.
- Accept non-JS names in the validator instead: fixes the messenger, not
  the tree; the output still cannot load as ESM.

Content-over-name ladder (future refinements, NOT part of this change):
when the content can be made to match the original name, prefer recovering
content over renaming — e.g. emit a real `data.json` when the module is a
pure JSON literal export, or extract css-loader string literals into a real
stylesheet plus a JS shim (analogous to `--vue-sfc`). Until such recovery
exists for a given shape, the naming rule above applies.

## Implementation notes

- Fix at the id→filename mapping (`sanitize_filename` in webpack4.rs /
  webpack5.rs, shared `sanitize_relative_path` callers). Import specifiers
  are synthesized from `id_to_filename`, so correcting the map corrects
  every consumer edge with no extra rewrite pass.
- Collisions (a bundle carrying both `./a.less` and `./a.less.js` ids) must
  go through the existing global filename-dedup machinery (old→final map,
  which also rewrites already-synthesized links).
- Same layer, same pass: strip loader query strings from string ids
  (`./App.vue?vue&type=style&index=0`) before filename derivation —
  `sanitize_relative_path` currently lets `?...` into filenames.
- Check whether the heuristic/scope-hoist and other paths that accept
  path-like names need the same normalization, or already append `.js`.
- Validator needs no change: `resolve_in_set` already falls back to
  `{target}.js`, and after this change emitted names are js-like anyway.
- Tests: synthetic module ids only (e.g. `./src/style/index.less`,
  `./widgets/Button.less` + `./widgets/Button.js`); cover the collision
  case, the query-string case, a `.ts` id staying unchanged, an
  extensionless id, and a full-output `validate_output_modules` clean run.
- Expect snapshot/reference churn for affected fixtures; review the diff as
  better-not-different, and run the private fixture suite.
