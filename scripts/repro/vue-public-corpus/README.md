# Vue Public Corpus

This is a manual confidence harness for `wakaru --vue-sfc` on pinned public Vue
builds. It clones public repositories into `target/`, builds them, runs Wakaru
against their generated JavaScript, and writes aggregate reports under
`target/vue-public-corpus/`.

It is intentionally not a committed snapshot source:

- Third-party source code is downloaded under `target/` and is not committed.
- Generated app bundles and recovered `.vue` files stay under `target/`.
- When a corpus run finds a bug, reduce it to a neutral synthetic regression
  test before committing anything.

## Run

```powershell
node scripts/repro/vue-public-corpus/run.mjs --list
node scripts/repro/vue-public-corpus/run.mjs
node scripts/repro/vue-public-corpus/run.mjs --case vite6-template-vue
node scripts/repro/vue-public-corpus/run.mjs --all
```

By default, the runner executes only cases with `"enabled": true` in
`cases.json`. Use `--all` or `--case <name>` for opt-in larger cases.

The runner builds `wakaru-cli` with the `dev-release` profile unless `WAKARU`
points at an existing binary:

```powershell
$env:WAKARU = "C:\path\to\wakaru.exe"
node scripts/repro/vue-public-corpus/run.mjs --no-build-wakaru
```

Useful repeat-run flags:

```powershell
node scripts/repro/vue-public-corpus/run.mjs --skip-install --skip-build
node scripts/repro/vue-public-corpus/run.mjs --refresh --case vite6-template-vue
node scripts/repro/vue-public-corpus/run.mjs --json
```

Reports:

- `target/vue-public-corpus/report.md`
- `target/vue-public-corpus/report.json`
- Per-case Wakaru outputs in `target/vue-public-corpus/outputs/<case>/`

## Metrics

The report counts Wakaru JSON module statuses:

- `recovered_vue_sfc`: recovered `.vue` artifact.
- `vue_sfc_source_js`: preserved JavaScript sibling for a recovered SFC.
- `vue_sfc_fallback_js`: likely Vue render module that did not recover.

It also scans recovered `.vue` files for `<!-- wakaru:` unsupported markers and
validates each recovered SFC with `@vue/compiler-sfc` parse/template compile.

## Adding Cases

Add entries to `cases.json` with pinned public refs. Prefer small defaults and
mark expensive cases with `"enabled": false`.

Required fields:

- `name`
- `repo`
- `ref`
- `install`
- `build`
- `inputs`

Optional fields:

- `subdir`: run install/build and resolve inputs from a subdirectory.
- `sparse`: sparse checkout paths for large repositories.
- `tier`, `bundler`, `notes`: report-only metadata.
