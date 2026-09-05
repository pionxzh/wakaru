# Bun single-file executables

"Single-file executable" is Bun's user-facing term for `bun build --compile`
output; this doc's filename and the internal format names keep Bun's internal
term ("standalone", as in `StandaloneModuleGraph`).

## What works

Wakaru can extract JavaScript from Bun single-file PE, Mach-O, and ELF
executables without running them:

```bash
wakaru ./compiled-app --unpack --raw -o raw/
wakaru ./compiled-app --unpack -o readable/
```

It can also extract every embedded file without transforming its bytes:

```bash
wakaru bun extract ./compiled-app -o extracted/
```

Bun writes a serialized module graph immediately before the fixed
`\n---- Bun! ----\n` trailer.
Wakaru works backward from that trailer, validates the offsets record, the
36-byte file records used by Bun 1.3.3–1.3.8 or the 52-byte records introduced
in Bun 1.3.9, and every referenced byte range. This avoids scanning for
printable JavaScript and does not require a separate native-section parser for
each operating system.

The normal `--unpack` CLI path extracts JS, JSX, TS, and TSX records. Bun may
already have combined many source modules into each record. Wakaru recognizes
Bun's compiled five-parameter CommonJS container and exposes its body to the
existing esbuild/Bun detector, which can recover finer factory and
scope-hoisted module regions. Those regions are useful bundle boundaries, but
are not guaranteed to match every original source file.

`wakaru bun extract` stops before that JavaScript pipeline. Every file record is
written byte-for-byte below `files/`, whether its loader is JavaScript, CSS,
file, JSON-family, TOML, WebAssembly, N-API, text, Bun shell, SQLite, HTML,
YAML, Markdown, or an unknown future loader. A deterministic `manifest.json`
preserves the record metadata and maps Bun's original virtual path to the safe
on-disk path.

The loader mapping and both record layouts follow Bun's `Loader` enum and
`CompiledModuleGraphFile` serializer. Bun declares the loader discriminants
append-only, so Wakaru keeps the raw numeric loader ID and extracts unknown IDs
instead of discarding their contents. The older layout has no module-info or
bytecode-origin fields, so those public slices and manifest regions are empty.
A changed or ambiguous record layout fails closed because the container has no
independent version field with which to select an unverified decoder.

## Internal intake and memory ownership

The normal CLI intake pushes JS/JSX/TS/TSX records into one `UnpackJob`,
with the entry point first. The parenthesized CommonJS container has the
exact parameters `exports`, `require`, `module`, `__filename`, and `__dirname`.
Exposing its body lets the esbuild/Bun detector recover inner factory and
scope-hoisted modules. This container layer is separate from JavaScript
bundle detection; see [unpacking.md](unpacking.md).

Single-file executable entry bodies can be tens of megabytes, so this handoff moves the
wrapper body into the detector instead of cloning it, restoring the body if
detection rejects the candidate. Once an esbuild/Bun factory shape is accepted,
the detector moves unresolved factory bodies into their pending output modules
while keeping the resolved analysis AST separate; emission must not reuse the
resolved AST because that can change binding names and synthesized imports.
Resolved support-declaration dependencies are reduced to compact binding sets,
allowing the full analysis AST to be dropped before scope splitting and module
emission. Top-level source declarations are indexed by position and cloned only
when a recovered module needs to own them; the detector must not retain one AST
clone per binding.

The public `wakaru::bun` API borrows exact content and metadata slices and
reports their absolute executable byte ranges. Opaque source-map, bytecode,
and module-info slices are available separately from JavaScript contents.

## Safety and validation

- Executable discovery is limited to explicit file paths; directory scans and
  stdin remain JavaScript-only inputs.
- Wakaru reads the executable as data and never runs it.
- A missing Bun trailer is reported as a non-match. A present but invalid graph
  is rejected instead of being partially extracted.
- Every embedded pointer is range-checked before the public API returns
  borrowed slices.
- Container extraction percent-encodes unsafe/non-UTF-8 path bytes, resolves
  existing symlinks, prevents case-insensitive collisions, and verifies every
  output remains below the selected directory.
- Suspicious graphs that request more than four times the executable size in
  output are rejected to limit pointer-alias amplification.

## Opaque internals

`wakaru bun extract --include-internals` writes three optional regions when
present: Bun's serialized source-map data, JavaScriptCore bytecode (including
serialized alignment padding), and ESM module information. These outputs live
under `internals/<record-index>/` and are also described in the manifest.

They are deliberately excluded by default because they are runtime
implementation data rather than source assets. Bun's serialized source map is
written as `source-map.bunmap`, not `.map`, because it is not v3 JSON.

## Current limits

- Bun's embedded source-map field uses an internal binary representation, not a
  v3 JSON source map, and is not used for name recovery.
- The parser supports the known Bun 1.3.3–1.3.8 and Bun 1.3.9-era record
  layouts. It intentionally rejects unknown or ambiguous layouts; another Bun
  layout change requires a new validated parser branch.
- Bun records do not preserve original filesystem permissions or symlink
  identities; extracted records are ordinary files.
- Some tightly connected scope-hoisted regions can remain coarser than the
  original source files.

See [public-api.md](public-api.md#surface-overview) for the borrowed
Rust API and [cli.md](cli.md) for both container extraction and normal unpack
options.
