# Conditional switch reproduction matrix

This matrix explores switch-like conditional decision trees emitted or
preserved by common minifiers. It is meant to capture concrete shapes before
adding `UnConditionals` rewrites.

The matrix includes standalone Terser plus SWC and esbuild output minified
through Terser so compound minification shapes are visible beside each tool's
direct output.

```bash
node scripts/repro/conditional-switch-matrix/matrix.mjs --level standard
node scripts/repro/conditional-switch-matrix/matrix.mjs --level standard --details
```

The script installs minifier packages under `target/repro-tools/`, so the first
run may download npm packages. `target/` is ignored by git.
