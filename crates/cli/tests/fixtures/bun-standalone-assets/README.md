# Bun standalone asset fixture

The `standalone*.bin` files are serialized module-graph tails from real Bun
standalone executables. They intentionally omit the platform runtime so the
fixtures remain small; Wakaru's parser accepts these bare serialized graphs.

Regenerate the current fixture with the repository's installed Bun:

```bash
bun generate.mjs
```

Generate a version-pinned fixture by running the script with that Bun binary
and an explicit output filename:

```bash
/path/to/bun-v1.3.3 generate.mjs standalone-v1.3.3.bin
/path/to/bun-v1.3.8 generate.mjs standalone-v1.3.8.bin
```

- `standalone-v1.3.3.bin`: Bun `1.3.3+274e01c73`
- `standalone-v1.3.8.bin`: Bun `1.3.8+b64edcb49`
- `standalone.bin`: Bun `1.3.13+bf2e2cecf`

The pinned fixtures exercise the first and last releases using the supported
36-byte file-record layout. Each graph contains one JavaScript entry and one
byte-exact file asset with NUL and non-UTF-8 bytes.
