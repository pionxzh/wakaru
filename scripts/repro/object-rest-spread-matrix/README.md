# Object Rest/Spread Reproduction Matrix

This harness checks how common tools lower object rest and object spread
snippets, then runs wakaru on the lowered output. Babel is tested across early
proposal plugins, current transform plugins, and the Babel 8 RC line, with spec,
loose, and `useBuiltIns` variants where supported.

The matrix also includes standalone Terser rows and Babel/TypeScript/SWC/esbuild
output minified through Terser, because object rest/spread helper shapes can
change again after compiler output is minified.

It is for investigation, not as a committed snapshot source. Use it to find
reproduced tool shapes worth minimizing into focused Rust unit tests.

```powershell
node scripts/repro/object-rest-spread-matrix/matrix.mjs
```

Add `--details` to print full lowered and recovered code for missed cases.
Add `--level minimal`, `--level standard`, or `--level aggressive` to run
wakaru with a specific rewrite level.

Every snippet also opts into the execution-equivalence check (see
`docs/testing.md`): the lowered program and the recovery run under `node:vm`
with identical stub environments and must produce the same effect log. The
`rest-mutated-binding` row exists specifically to pin the declaration-kind
contract — a recovery that emits `const` for a later-reassigned rest binding
fails there with `behavior diverged` even though every needle matches. Shapes
lowered to module syntax (helper imports) skip the execution check.

Rows are grouped by distinct lowered output per snippet. The grouping key only
normalizes CRLF to LF and trims leading/trailing whitespace, so exact helper
shape is still preserved while duplicate tool outputs are collapsed.

By default the script uses `target/debug/wakaru(.exe)` when present, otherwise
it falls back to `cargo run -q -p wakaru-cli --`. Set `WAKARU` to test a
specific binary.

The transformer packages are installed under `target/repro-tools/`, so the first
run may download Babel, TypeScript, SWC, esbuild, or Terser packages. The
`target/` directory is ignored by git.
