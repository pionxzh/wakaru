# Hand-authored webpack fixtures

Bundles written by hand, not produced by a bundler. Their shapes are
`hypothetical` (see `docs/reviewing.md#shape-claims-need-provenance`): they
test wakaru's behavior on inputs no generator here emits. Generated webpack
fixtures live in `../webpack-gen/`.

- `wp-path-traversal.js`: a webpack 4-style module table whose string ids
  contain `../` and backslash segments. Unpacking must never emit a filename
  that escapes the output directory.
- `wp5-require-s.js`: a webpack 5-style runtime that starts the entry through
  `__webpack_require__(__webpack_require__.s = 2)`.
