# wakaru-rs

Rust rewrite of Wakaru's unminify core.

## Status

Implemented:

- Parse source into SWC AST
- Run resolver
- Run rule pipeline:
  - `SimplifySequence` (`a(), b(), c()` -> `a(); b(); c();`)
  - `FlipComparisons` (`null == x` -> `x == null`)
  - `RemoveVoid` (`void 0` -> `undefined`)
  - `UnminifyBooleans` (`!0`/`!1` -> `true`/`false`)
  - `UnInfinity` (`1 / 0` -> `Infinity`, `-1 / 0` -> `-Infinity`)
  - `UnTypeof` (`typeof x < "u"` -> `typeof x !== "undefined"`)
  - `UnTemplateLiteral` (`"a".concat(b)` -> `` `a${b}` ``)
  - `UnBracketNotation` (`obj['foo']` -> `obj.foo`, `obj['1']` -> `obj[1]`)
  - `UnReturn` (remove redundant tail `return undefined` / `return;`, convert tail `return void expr`)
  - `UnUseStrict` (remove `'use strict'` directives)
- Run hygiene + fixer
- Print readable JavaScript output

## CLI

```bash
cargo run --bin wakaru-rs -- path/to/input.js -o path/to/output.js
```

## Tests

Integration tests reuse bundled fixtures from `../testcases/*/dist/index.js`.
