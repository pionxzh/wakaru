# Bun graph fixtures with UTF-16 records

`*-graph.bin` files are serialized module graphs cut out of real Bun
single-file executables: the bytes from the start of the graph data through the
`\n---- Bun! ----\n` trailer, without the platform runtime. Wakaru's parser
accepts these bare graphs; prepend `\x7fELF` to pass one to `--unpack`, which
only treats files with an executable magic as containers.

The project directory next to each graph holds the files it was built from.

## `utf16-graph.bin`

Producer `bun@1.4.3 build --compile --minify --sourcemap=linked`, entry
`src/index.ts`, run in `utf16/`. The entry's virtual path is
`/$bunfs/root/app14`.

- `src/index.ts` keeps a `/*! © ... */` legal comment, so Bun stores the
  JavaScript entry as UTF-16 (encoding byte 2).
- `src/note.txt` is a non-ASCII `text` import, also stored as UTF-16. Its
  extracted UTF-8 transcode must equal this file byte-for-byte.
