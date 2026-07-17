<div align="center">

# Wakaru

**Unpack. Unminify. Understand.**

Wakaru unpacks webpack, esbuild, and other production bundles, then reverses
minifier and transpiler output into readable modern JavaScript.

[![CI](https://img.shields.io/github/actions/workflow/status/pionxzh/wakaru/rust-ci.yml?branch=main&label=CI)](https://github.com/pionxzh/wakaru/actions/workflows/rust-ci.yml)
[![npm](https://img.shields.io/npm/v/@wakaru/cli?label=npm)](https://www.npmjs.com/package/@wakaru/cli)
[![Telegram](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

[**Try it in the playground**](https://wakaru.vercel.app/playground) — paste minified JavaScript, get readable code back.

</div>

## What it does

A formatter only changes whitespace and layout. Wakaru rewrites the JavaScript
AST to reverse minifier artifacts, restore transpiled syntax, remove bundler
runtimes, and split bundles back into modules.

Feed Wakaru this minified Babel output:

```js
"use strict";Object.defineProperty(exports,"__esModule",{value:!0}),exports.loadProfile=void 0;
var _api=_interopRequireDefault(require("./api"));
function _interopRequireDefault(e){return e&&e.__esModule?e:{default:e}}
function _asyncToGenerator(e){return function(){var t=this,r=arguments;return new Promise(function(n,o){var a=e.apply(t,r);function i(e){c(a,n,o,i,u,"next",e)}function u(e){c(a,n,o,i,u,"throw",e)}i(void 0)})}}
function c(e,t,r,n,o,a,i){try{var u=e[a](i),c=u.value}catch(e){return void r(e)}u.done?t(c):Promise.resolve(c).then(n,o)}
var loadProfile=function(){var e=_asyncToGenerator(function*(e){var t=yield _api.default.fetchUser(e),r=null!=t.name?t.name:"anonymous";return{name:r,avatar:null==t.profile?void 0:t.profile.avatar}});return function(t){return e.apply(this,arguments)}}();exports.loadProfile=loadProfile;
```

and get this back:

```js
import _api from "./api";
export const loadProfile = async (e)=>{
    const t = await _api.fetchUser(e);
    const name = t.name ?? "anonymous";
    return {
        name,
        avatar: t.profile?.avatar
    };
};
```

That is real, unedited output: the runtime helpers are gone, `async`/`await`
is recovered from the generator state machine, `??` and `?.` are restored,
and the module is ESM again. (Wakaru applies conservative renaming heuristics
where the code gives evidence, but most mangled locals like `e` stay short
unless the source map includes original names.)

## Quick start

```bash
npx @wakaru/cli input.js -o output.js               # decompile a file
npx @wakaru/cli bundle.js --unpack -o out/          # unpack and decompile a bundle
npx @wakaru/cli dist/ --unpack -o out/              # scan a bundle output directory
```

Full flag reference: [docs/cli.md](./docs/cli.md).

## What it handles

- **Bundle splitting** — webpack 4/5 (including Vercel ncc CommonJS/IIFE
  output), esbuild, Bun, Browserify (including Cocos Creator 2.x project-script
  bundles), SystemJS, AMD/UMD, plus heuristic splitting of scope-hoisted ESM
  output (Rollup, Vite).
- **Transpiler recovery** — Babel, TypeScript/tslib, and SWC runtime helpers:
  async/await from generator state machines, classes, spread/rest, enums,
  JSX, template literals, optional chaining, nullish coalescing, default
  parameters, `for...of`, and more.
- **Minifier recovery** — sequence expressions, flipped comparisons,
  `!0`/`void 0` literal tricks, IIFE flattening, alias inlining.
- **Three rewrite levels** — `minimal` (highest-confidence,
  semantics-preserving transforms for auditing and diffing), `standard`
  (default), `aggressive` (maximum readability). The semantic contract per
  level is documented in
  [rewrite-assumptions.md](./docs/rewrite-assumptions.md).

## Tested like a compiler

Wakaru restores structure while respecting JavaScript semantics:

- **62,061 passing Test262 semantic round trips, with zero Wakaru correctness
  failures.** The canonical 3-producer × 20-slice matrix contains 66,729
  runnable inputs; 4,668 are classified as unsupported or rejected rather
  than counted as passes. A pass preserves the typed Test262
  expectation; positive and runtime/resolution-negative cases run the
  original source, transformed/minified source, and Wakaru's output through
  the same harness.
  Separate canonical baselines cover multi-file ESM module graphs. See
  [test262-roundtrip.md](./docs/test262-roundtrip.md) and the current
  [`test262-stats.json`](./scripts/correctness/test262-stats.json).
- **96.4% pattern recovery across 1,743 transpiler × minifier test shapes.**
  Reproduction matrices compile known inputs through real Babel/TypeScript/
  SWC/esbuild/Terser version combinations and verify Wakaru recovers the
  original construct. Current rates per matrix:
  [`scripts/repro/stats.json`](./scripts/repro/stats.json).
## Works with other tools

**Obfuscated input?** Wakaru is deliberately not a deobfuscator — heavy
obfuscation (string arrays, control-flow flattening, VM-based protectors) is
a different problem. Strip it first with a dedicated tool like
[webcrack](https://github.com/j4k0xb/webcrack), then let Wakaru recover the
readable modules:

```bash
npx webcrack --no-unpack --no-unminify obfuscated.js > deobfuscated.js  # 1. strip the obfuscation
npx @wakaru/cli deobfuscated.js --unpack -o out/                        # 2. recover readable modules
```

**Want better names?** Pair Wakaru's deterministic structure recovery with an
LLM renamer like [humanify](https://github.com/jehna/humanify), or use
`--source-map` when the map includes original names.

## Use cases

- **Security review & bug bounty** — read what a site actually ships instead
  of scrolling one 5 MB line. Split the bundle, find the first-party code,
  audit it as modules.
- **Incident response & malware triage** — unminify a suspicious script into
  something a human can diff and reason about, using `minimal` level to favor
  behavioral fidelity.
- **Recovering lost source** — the vendor vanished, the laptop died, and all
  that's left is `dist/`. Reconstruct a workable codebase from the bundle
  (and `wakaru extract` recovers originals when the map includes
  `sourcesContent`).
- **Debugging third-party SDKs** — turn the vendored blob into readable
  modules so the stack trace points at code you can actually understand.
- **Supply-chain inspection** — see what's inside a dependency's shipped
  bundle rather than trusting the repo it claims to be built from.

## Use it from an agent

Coding agents hit unreadable minified JS constantly. Wakaru ships a
[`SKILL.md`](./SKILL.md) — drop it into Claude Code, Codex, Grok, or any
agent that reads skills, and the agent knows when and how to unpack a bundle,
read the recovered modules like ordinary source, and pick the right rewrite
level.

## Install

```bash
npm install -g @wakaru/cli@latest
```

Or pre-built binaries from [GitHub Releases](https://github.com/pionxzh/wakaru/releases).
Full CLI documentation: [docs/cli.md](./docs/cli.md).

## Contributing

Contributions are welcome, especially:

- Share real-world bundles that Wakaru doesn't handle well
- Report missing helper detection or false positives
- Report semantic or correctness issues

When reporting a bug, please include: the input code, the command you ran, the current output, and what you expected instead.

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for development setup and PR checks. Project docs start at [`docs/README.md`](./docs/README.md).

## License

[Apache-2.0](./LICENSE)

<sub>Usage of Wakaru for attacking targets without prior mutual consent is illegal. End users are responsible for complying with all applicable laws.</sub>
