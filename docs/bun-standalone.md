# Bun standalone executables

## What works

Wakaru can extract JavaScript from Bun standalone PE, Mach-O, and ELF
executables without running them:

```bash
wakaru ./compiled-app --unpack --raw -o raw/
wakaru ./compiled-app --unpack -o readable/
```

Bun writes a serialized module graph immediately before a fixed trailer.
Wakaru works backward from that trailer, validates the offsets record, the
current 52-byte file records, and every referenced byte range. This avoids
scanning for printable JavaScript and does not require a separate native-section
parser for each operating system.

The CLI extracts JS, JSX, TS, and TSX records. Bun may already have combined
many source modules into each record. Wakaru recognizes Bun's compiled
five-parameter CommonJS container and exposes its body to the existing
esbuild/Bun detector, which can recover finer factory and scope-hoisted module
regions. Those regions are useful bundle boundaries, but are not guaranteed to
match every original source file.

## Safety and validation

- Executable discovery is limited to explicit file paths; directory scans and
  stdin remain JavaScript-only inputs.
- Wakaru reads the executable as data and never runs it.
- A missing Bun trailer is reported as a non-match. A present but invalid graph
  is rejected instead of being partially extracted.
- Every embedded content and source-map pointer is range-checked before the
  public API returns borrowed slices.
- Binary assets are available through the Rust API but are not emitted by the
  CLI.

## Current limits

- Bun's embedded source-map field uses an internal binary representation, not a
  v3 JSON source map, and is not used for name recovery.
- The parser intentionally rejects unknown record layouts. A future Bun layout
  change may require a new validated parser branch.
- Some tightly connected scope-hoisted regions can remain coarser than the
  original source files.

See [public-api.md](public-api.md#bun-standalone-extraction) for the borrowed
Rust API and [cli.md](cli.md) for normal unpack options.
