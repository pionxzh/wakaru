# For-of iteration reproduction matrix

This matrix compares array `for...of`, iterable `for...of`, and destructuring
loop-head lowerings across TypeScript, Babel, and SWC. It is intended to expose
whether `UnForOf` recovers the major index-loop and iterator-helper shapes.

```bash
node scripts/repro/for-of-iteration-matrix/matrix.mjs --level standard
node scripts/repro/for-of-iteration-matrix/matrix.mjs --level standard --details
```

The script installs transformer packages under `target/repro-tools/`, so the
first run may download npm packages. `target/` is ignored by git.
