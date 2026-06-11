# AMD and UMD unpack reproduction matrix

This harness builds small AMD and UMD bundles with real tools, then runs wakaru
with `--unpack` to verify the expected module extraction shape.

It focuses on issue-style repros instead of a broad synthetic variant matrix:

- RequireJS optimizer output with multiple named `define(...)` modules
- Rollup AMD output with a named `define(...)` factory
- Rollup UMD output with a plain single-module UMD wrapper

```powershell
node scripts/repro/amd-umd-unpack-matrix/matrix.mjs
```

Set `WAKARU` to test a specific binary. By default the script uses
`target/debug/wakaru(.exe)` when present, otherwise it falls back to
`cargo run -q -p wakaru-cli --`.

The tool packages are installed under `target/repro-tools/`, so the first run
may download RequireJS or Rollup packages. The `target/` directory is ignored by
git.
