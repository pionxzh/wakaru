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

It can also extract every embedded file as the bundler emitted it, plus the
original source files when the build embedded a source map:

```bash
wakaru bun extract ./compiled-app -o extracted/
```

Bun writes a serialized module graph immediately before the fixed
`\n---- Bun! ----\n` trailer.
Wakaru works backward from that trailer, validates the offsets record, the
36-byte file records used by Bun 1.3.3–1.3.8 or the 52-byte records introduced
in Bun 1.3.9 (unchanged through Bun 1.4.3), and every referenced byte range. This avoids scanning for
printable JavaScript and does not require a separate native-section parser for
each operating system.

The normal `--unpack` CLI path extracts JS, JSX, TS, and TSX records. Bun may
already have combined many source modules into each record. Wakaru recognizes
Bun's compiled five-parameter CommonJS container and exposes its body to the
existing esbuild/Bun detector, which can recover finer factory and
scope-hoisted module regions. Those regions are useful bundle boundaries, but
are not guaranteed to match every original source file.

`wakaru bun extract` stops before that JavaScript pipeline. Every file record is
written below `files/`, whether its loader is JavaScript, CSS, file,
JSON-family, TOML, WebAssembly, N-API, text, Bun shell, SQLite, HTML, YAML,
Markdown, or an unknown future loader. A record is written byte-for-byte as Bun
stored it, except a record Bun stored as UTF-16 (see
[Text encodings](#text-encodings)), which is written as the bytes Bun encoded
it from. A deterministic `manifest.json` preserves the record metadata, the
stored byte range, and maps Bun's original virtual path to the safe on-disk
path.

The loader mapping and both record layouts follow Bun's `Loader` enum and
`CompiledModuleGraphFile` serializer. Bun declares the loader discriminants
append-only, so Wakaru keeps the raw numeric loader ID and extracts unknown IDs
instead of discarding their contents. The older layout has no module-info or
bytecode-origin fields, so those public slices and manifest regions are empty.
A changed or ambiguous record layout fails closed because the container has no
independent version field with which to select an unverified decoder.

## Text encodings

The record's encoding byte says how `contents` is stored. Bun 1.4.0 and
earlier label JavaScript records `latin1` (1) while storing UTF-8 bytes, and
assets `binary` (0); they declare 2 as UTF-8 but never write it. Bun 1.4.1
reuses 2 for UTF-16 (`Encoding::Utf16` in `StandaloneModuleGraph`): a
server-side JavaScript chunk or a `text`-loader import is stored as a
JavaScriptCore string body. ASCII-only text stays `latin1` with one NUL
terminator. Any other text is transcoded from UTF-8 (WTF-8) to little-endian
UTF-16 at an even offset, with an even byte length and two NUL bytes after it;
Bun pads one byte before it when needed. Bun's runtime presents such a file to
the program as its UTF-8 transcode.

Wakaru validates the offset parity relative to the graph data, so a bare graph
cut out of an executable validates the same way, plus the byte-length parity
and both terminator bytes. `BunEmbeddedFile::file_bytes` returns the stored
bytes, or for UTF-16 the transcode back to the bytes Bun encoded (WTF-8, which
is UTF-8 unless the text holds an unpaired surrogate). `BunEmbeddedFile::text`
decodes strictly and fails on invalid UTF-8 or an unpaired surrogate; the
`--unpack` intake uses it. `wakaru bun extract` writes `file_bytes` and labels
the record `utf16le` in the manifest, where `bytes` and `executable_range`
still describe the stored UTF-16 and `output_bytes` the written length.

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
reports their absolute executable byte ranges; only the transcode of a UTF-16
record and a decompressed original source are owned. Opaque source-map,
bytecode, and module-info slices are available separately from JavaScript
contents.

## Original sources

When the build passes `--sourcemap` (any value was observed to embed one,
including `none` on Bun 1.3.13), each JavaScript record carries Bun's
`SerializedSourceMap`. Bun's writer
(`serializeJsonSourceMapForStandalone`) refuses a map without
`sourcesContent`, or whose `sources` and `sourcesContent` lengths differ, so
every embedded path has its file's text. The layout has been stable since at
least Bun 1.2; all integers are little-endian and every `StringPointer` is
relative to the start of the map:

```text
u32 source_files_count, u32 map_bytes_length    SerializedSourceMapHeader
source_files_count × StringPointer              sources paths (UTF-8)
source_files_count × StringPointer              sourcesContent, each one zstd frame
map_bytes_length bytes                          mappings
string payload                                  the paths, then the frames
```

The mapping bytes changed representation (VLQ text in older versions,
`InternalSourceMap` from 1.3.13), so Wakaru does not read them.
`BunEmbeddedFile::original_sources` reads the header and pointers on demand;
`extract_standalone` does not, so a map it cannot read never rejects the
graph. It requires the pointer table to fit the map, the string payload to
start at `8 + 16 × count + map_bytes_length` within the map, the first path to
start there (Bun's writer asserts this), every pointer to stay inside the
payload, and every path to be UTF-8. A count of zero is no sources.

`BunOriginalSource::decompress(limit)` decodes one frame with the pure-Rust
`ruzstd` decoder. A frame that declares a content size over `limit` fails
before decoding; one that decodes past `limit` fails when it does; trailing
bytes, a declared size that does not match, or a checksum mismatch also fail.
Decompressed files are the original pre-bundle files, byte-identical (for
example TypeScript with its types and comments).

`wakaru bun extract` writes them below `sources/` by default; they are source
files, not runtime internals. A path is the bundler's, usually relative to the
build's working directory: both `/` and `\` separate components, empty and
`.` components are dropped, and `..` becomes `%2E%2E`, so
`../node_modules/pkg/x.js` is written to
`sources/%2E%2E/node_modules/pkg/x.js`. Other unsafe bytes are
percent-encoded as in `files/`. A path with no components is written as
`sources/source-<record>-<index>`. When several records carry the same path
with identical bytes, it is written once; different bytes under one path get
the numeric suffix used for case-insensitive collisions. The manifest's
`sources` list has one entry per occurrence, with the record index, position in
the map, original path, output path, compressed size, compressed executable
range, and decompressed size. Unlike the graph, an unreadable map does not
fail the extraction: the embedded files do not depend on it, and its format is
not part of what the graph validates. When a record's map cannot be read, one
of its frames is corrupt, or its sources would pass the remaining extraction
budget, none of that record's sources are written; the files and other
records' sources still are. Each skipped record prints a stderr warning and
gets an entry in the manifest's `skipped_sources` list (`record` and
`message`).

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
  output are rejected to limit pointer-alias amplification. A UTF-16 record
  counts at its largest possible transcode, 1.5 times its stored size, before
  any transcode allocates. Original sources are decompressed within what is
  left of that budget; every decompression counts, including a repeated file
  that is written once.

## Opaque internals

`wakaru bun extract --include-internals` writes three optional regions when
present: Bun's whole serialized source map, JavaScriptCore bytecode (including
serialized alignment padding), and ESM module information. These outputs live
under `internals/<record-index>/` and are also described in the manifest.

They are deliberately excluded by default because they are runtime
implementation data rather than source assets; the original sources the map
carries are written below `sources/` without this flag. Bun's serialized
source map is written as `source-map.bunmap`, not `.map`, because it is not v3
JSON.

## Current limits

- Bun's embedded source map is an internal binary representation, not a v3
  JSON source map. Wakaru recovers its original sources but does not decode
  its mappings, so they are not used for name recovery or position mapping.
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
