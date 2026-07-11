# Optional/Nullish Reproduction Matrix

This harness checks how common tools lower optional chaining, nullish
coalescing, and nullish assignment snippets, then runs wakaru on the lowered
output. Babel is tested across a few meaningful lines: early proposal plugins,
assumptions-era Babel 7, current transform plugins, and the Babel 8 RC line.

The matrix also includes standalone Terser rows and Babel/TypeScript/SWC/esbuild
output minified through Terser, because optional/nullish lowering can become a
different recoverable shape after minification. The nullish-assignment rows
cover identifier, static-member, and side-effectful computed-member targets.
The computed-member rows verify that compiler object/key temporaries are folded
back into `getTarget()[getKey()] ??= make()` when those temporaries prove the
receiver and key are each evaluated once.

The logical-AND rows include issue #166-style boolean prefixes, such as a
lowered optional chain followed by ordinary suffix conditions or a second
lowered optional access.

It is for investigation, not as a committed snapshot source. Use it to find
reproduced tool shapes worth minimizing into focused Rust unit tests.

```powershell
node scripts/repro/optional-nullish-matrix/matrix.mjs
```

Add `--details` to print full lowered and recovered code for missed cases.
Add `--level minimal`, `--level standard`, or `--level aggressive` to run
wakaru with a specific rewrite level.

Some Babel loose optional-call rows are expected to stay unrecovered at
`standard`, including the same loose output after Terser: those lowerings read
the same property twice, while optional-call syntax reads it once. Wakaru only
recovers those rows at `aggressive`, where stable getter reads are an accepted
assumption. The matrix marks those rows as `gated` instead of `no`.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

By default the script uses `target/debug/wakaru(.exe)` when present, otherwise
it falls back to `cargo run -q -p wakaru-cli --`. Set `WAKARU` to test a
specific binary.

The transformer packages are installed under `target/repro-tools/`, so the first
run may download Babel, TypeScript, SWC, esbuild, or Terser packages. The
`target/` directory is ignored by git.
