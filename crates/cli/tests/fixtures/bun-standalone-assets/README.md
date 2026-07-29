# Bun standalone asset fixture

`standalone.bin` is the serialized module-graph tail from a real Bun standalone
executable. It intentionally omits the platform runtime so the fixture remains
small; Wakaru's parser accepts this bare serialized graph.

Regenerate with the repository's installed Bun:

```bash
bun generate.mjs
```

The fixture was last generated with Bun 1.3.13
(`1.3.13+bf2e2cecf`). It contains one JavaScript entry and one byte-exact file
asset with NUL and non-UTF-8 bytes.
