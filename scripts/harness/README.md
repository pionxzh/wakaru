# Synthetic-bundle harness

Generates seeded synthetic apps, builds them across a bundler × mode × layout matrix,
and verifies wakaru unpacks each variant. It exercises real bundler output
shapes the fixture corpus does not systematically cover: esbuild
`--format=iife` (the browser default) and `--splitting`, webpack5 prod/dev,
rollup single-file iife and code-split esm.

## What it does

Per fixture (seeded synthetic app) and matrix variant:

1. `gen_app.py` — deterministic app: layered-DAG ESM modules (plus ~25% CJS
   modules and dynamic-import roots, so app modules survive prod builds as
   separate units) importing a stratified sample of npm packages from
   `pool.json`
2. build with the bundler (source maps on)
3. `wakaru --unpack --provenance`
4. group outputs by physical input using `provenance.json` and require at
   least one input to expand into multiple logical modules (exit 1 on any
   build, unpack, or split failure)

## Usage

```bash
cd scripts/harness
python3 run_harness.py --seeds 1,2,3 --bundlers esbuild,webpack5,rollup \
  --modes prod,dev --layouts iife,split
```

First run does a one-time `npm install` of the package pool into
`workspace/` (gitignored). Requires a `dev-release` CLI build (or pass
`--wakaru`).

## Notes

- esbuild and rollup run both `iife` and `split` layouts. Webpack runs once per
  mode because its generated app already exercises async chunks.
- The npm pool resolves versions at install time; `workspace/package-lock.json`
  freezes them for reproducibility. Delete `workspace/node_modules` to
  re-resolve.
- Found-by-harness: esbuild `--format=iife` bundles were not unpacked at all
  until the plain-IIFE unwrap in `crates/core/src/unpacker/wrappers.rs`
  (tests in `crates/core/tests/esbuild_unpack.rs`).
- Package-detection evaluation on these fixtures (ground truth, eval metrics)
  lives in the detector research repo, not here.
