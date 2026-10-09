# CommonJS Export-Storage Matrix

This matrix tests whether a module that a producer compiled from ESM to
CommonJS decompiles back to ESM with the **same runtime behavior**. It targets
`UnEsm`: how the recovered module stores and exposes each export, and whether
reads and writes of that export still work after `exports` is gone.

Unlike the other matrices, it compares behavior rather than output shape.
Text or structure comparison cannot see the failures this matrix exists for:
a leftover `exports.count += 1` inside a function looks harmless and throws
`ReferenceError` only when it runs, and a snapshot `export const` looks like a
live export until a later write is missed.

The matrix is registered in `collect-stats.mjs`, so its counts are part of
`stats.json` and the aggregate recovery rate. A change to `UnEsm` or the
interop helpers that moves a row shows up in `collect-stats.mjs --check`.

## How a row is judged

Each case in `cases.mjs` is an ESM module (`mod.js`, sometimes with a
`dep.js` it imports) plus a driver. The driver receives the module namespace
`m` and records observations with `log`: export values before and after
calling exported mutators, return values, `instanceof` checks, thrown errors.

For every case and producer:

1. Run the driver against the original ESM. This is the expected log.
2. Compile every file to CommonJS and run the driver against
   `require("./mod.js")`. If the log differs from step 1, the producer itself
   did not preserve ESM semantics (or emitted output that does not parse).
   The row is `p≠` and is excluded from the score.
3. Decompile every CommonJS file with `wakaru <file> -o <out>` and run the
   driver against the recovered ESM. The row is `ok` when the log matches
   step 1, and `no` otherwise. A wakaru failure is `err` and counts as `no`.

A bundler profile builds one bundle per case instead. A stub entry imports
the namespace of `mod.js` and stores it on a global, so `mod.js` is an
ordinary module of the bundle, not the entry. Step 2 runs the bundle and
reads the global; step 3 unpacks the bundle with `wakaru <bundle> --unpack`,
runs the unpacked `entry.js`, and reads the same global.

Single-file decompilation is deliberate for the compiler profiles: it is what
a user gets for one published CommonJS file. Cases with a `dep.js` decompile each file
separately, so they also expose single-file interop limits (for example a
default import synthesized for a module that has no default export).

## Producer profiles

| Profile | Producer | Options |
|---|---|---|
| `tsc-5.9-es2020`, `tsc-5.9-es5` | TypeScript 5.9.3 `transpileModule` | `module: CommonJS`, `esModuleInterop` |
| `tsc-4.3-es2015` | TypeScript 4.3.5 | same |
| `tsc-3.9-es5` | TypeScript 3.9.10 | same |
| `babel-7.28`, `babel-7.28-loose` | `@babel/core` 7.28.5 + `@babel/plugin-transform-modules-commonjs` 7.28.6 | default / `loose: true` |
| `swc-1.16-es2020`, `swc-1.16-es5` | `@swc/core` 1.16.2 (shared pin) | `module.type: "commonjs"` |
| `esbuild-0.28` | esbuild 0.28.0 `transformSync` | `format: "cjs"`, target es2020 |
| `rollup-4.63` | rollup 4.63.5 | `format: "cjs"`, every other module external |
| `sucrase-3.35` | sucrase 3.35.1 | `transforms: ["imports"]` |
| `webpack-5.107-terser` | webpack 5.107.2 | `mode: production` (Terser), `concatenateModules: false`, one chunk |
| `webpack-5.111`, `webpack-5.111-terser` | webpack 5.111.1 | same, without and with minification |

The producers cover the three ways an ESM export is stored in CommonJS:
the `exports` property itself (TypeScript, rollup), a local binding mirrored
into the property on every write (Babel, sucrase, TypeScript aliases), and a
local binding exposed through a getter (swc, esbuild, webpack). The proposal
lists the exact shapes. webpack 5.108 and later emit an array form of
`require.d` for `const` exports; 5.107 is the last release with only the
object form, so the two webpack versions cover both.

## Running

```bash
cargo build -p wakaru-cli
node scripts/repro/cjs-export-storage-matrix/matrix.mjs            # table
node scripts/repro/cjs-export-storage-matrix/matrix.mjs --details  # logs and leftovers for non-ok rows
node scripts/repro/cjs-export-storage-matrix/matrix.mjs --json
node scripts/repro/cjs-export-storage-matrix/matrix.mjs --case counter --producer babel --keep
node scripts/repro/cjs-export-storage-matrix/matrix.mjs --explain  # per-name storage decisions
```

`--case` and `--producer` filter by substring. `--keep` leaves the work
directory (original ESM, CommonJS, and recovered ESM for every row) in place
and prints its path. `WAKARU=<binary>` selects the binary, as for every other
matrix; comparing two binaries is two runs.

`--details` prints, for every row that is not `ok`, the three logs and each
recovered line that still mentions `exports`, `module.exports`, or
`require`. Those lines are a debugging aid, not part of the verdict: an export
named `module` legitimately prints that word.

`--explain` also runs `wakaru debug cjs-exports` on every CommonJS file and
prints each name's storage decision (`getter`, `mirror`, `property`,
`unrecovered`, or the failed module gate) with the rejected models in
brackets, for every row that reached decompilation.

The table output ends with how many `wrong` and `ok` rows got a
`commonjs_export_unrecovered` warning from wakaru, and `--details` prints the
warning for each row. The warning should never fire on an `ok` row. A `wrong`
row without it is either output that stayed CommonJS (valid CommonJS, but the
ESM driver cannot load it) or a recovered ESM module whose export surface is
wrong without any `exports` access left.

## Adding a case

Add an entry to `cases.mjs`. Keep sources small and synthetic, and make the
driver observe the behavior the case is about: call the mutator, then read the
export through `m`. Reading `m.x` after a mutation is what distinguishes a
live export from a snapshot. Every producer runs every case; a case that a
producer cannot express shows up as `p≠` or `c-err`, not as a wakaru failure.

Cases come from producers only. Hand-written CommonJS stays out of the matrix:
it is rare in this role, it often should stay CommonJS, and it is judged case
by case (export-storage decision 2 in `docs/proposals/cjs-export-storage.md`).
