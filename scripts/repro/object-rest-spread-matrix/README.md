# Object Rest/Spread Reproduction Matrix

This harness checks how common tools lower object rest and object spread
snippets, then runs wakaru on the lowered output. Babel is tested across early
proposal plugins, current transform plugins, and the Babel 8 RC line, with spec,
loose, and `useBuiltIns` variants where supported.

It is for investigation, not as a committed snapshot source. Use it to find
reproduced tool shapes worth minimizing into focused Rust unit tests.

```powershell
node scripts/repro/object-rest-spread-matrix/matrix.mjs
```

Add `--details` to print full lowered and recovered code for missed cases.
Add `--level minimal`, `--level standard`, or `--level aggressive` to run
wakaru with a specific rewrite level.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

By default the script uses `target/debug/wakaru(.exe)` when present, otherwise
it falls back to `cargo run -q -p wakaru-cli --`. Set `WAKARU` to test a
specific binary.

The transformer packages are installed under `target/repro-tools/`, so the first
run may download Babel, SWC, or esbuild packages. The `target/` directory is
ignored by git.
