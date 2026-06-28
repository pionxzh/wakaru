# @wakaru/cli

Fast JavaScript decompiler and bundle splitter for modern frontend code.

## Quick Start

```bash
npx @wakaru/cli input.js -o output.js
npx @wakaru/cli bundle.js --unpack -o out/
npx @wakaru/cli dist/ --unpack -o out/
```

## Install

```bash
npm install -g @wakaru/cli@latest
wakaru input.js -o output.js
```

## What It Does

- Splits bundles from webpack 4/5, esbuild, Bun, Browserify, SystemJS, and AMD.
- Recovers readable JavaScript from transpiler and minifier output.
- Supports source maps for name recovery and output mappings.
- Offers `minimal`, `standard`, and `aggressive` rewrite levels.

## Links

- Repository: https://github.com/pionxzh/wakaru
- Documentation: https://github.com/pionxzh/wakaru#readme
- Releases: https://github.com/pionxzh/wakaru/releases
