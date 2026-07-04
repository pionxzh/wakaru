<div align="center">

# Wakaru

**Unpack. Unminify. Understand.**

Wakaru turns production JavaScript — bundled, transpiled, minified — back into
readable modules. It is the JS decompiler with receipts: the only one backed
by a semantic-equivalence test suite.

[![CI](https://img.shields.io/github/actions/workflow/status/pionxzh/wakaru/rust-ci.yml?branch=main&label=CI)](https://github.com/pionxzh/wakaru/actions/workflows/rust-ci.yml)
[![npm](https://img.shields.io/npm/v/@wakaru/cli?label=npm)](https://www.npmjs.com/package/@wakaru/cli)
[![Telegram](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

[**Try it in the playground**](https://wakaru.vercel.app/playground) — paste a bundle, get modules back.

</div>

## What it does

Feed it this — minified Babel output, straight from a production bundle:

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
and the module is ESM again. (Mangled locals like `e` stay mangled unless a
source map is available — Wakaru recovers structure deterministically and
never invents names.)

## Quick start

```bash
npx @wakaru/cli input.js -o output.js               # decompile a file
npx @wakaru/cli bundle.js --unpack -o out/          # unpack and decompile a bundle
npx @wakaru/cli dist/ --unpack -o out/              # scan a bundle output directory
```

Full flag reference: [docs/cli.md](./docs/cli.md).

## What it handles

- **Bundle splitting** — webpack 4/5 (including chunks and multi-file
  entry+chunk sets), esbuild, Bun, Browserify, SystemJS, AMD/UMD, plus
  heuristic splitting of scope-hoisted ESM output (Rollup, Vite).
- **Transpiler recovery** — Babel, TypeScript/tslib, and SWC runtime helpers:
  async/await from generator state machines, classes, spread/rest, enums,
  JSX, template literals, optional chaining, nullish coalescing, default
  parameters, `for...of`, and more (~60 restoration rules).
- **Minifier recovery** — sequence expressions, flipped comparisons,
  boolean/number/`void 0` encodings, IIFE flattening, alias inlining.
- **Source maps** — original-name recovery and import deduplication when a
  map is available; `wakaru extract` dumps embedded `sourcesContent` to disk.
- **Three rewrite levels** — `minimal` (near-zero semantic change, for
  auditing and diffing), `standard` (default), `aggressive` (maximum
  readability). The semantic contract per level is documented in
  [rewrite-assumptions.md](./docs/rewrite-assumptions.md).

## Tested like a compiler

Claims in this space are cheap, so Wakaru ships its evidence:

- **0 semantic failures across 41,000+ runnable Test262 round-trip cases.**
  Each case runs the original source, a transformed/minified version, and
  Wakaru's decompiled output through the same Test262 harness — all three
  must pass. Covers 3 producer pipelines (Terser, SWC, esbuild) × 20 feature
  slices, plus multi-file ESM module graphs. Cases blocked by upstream
  parser/printer/transform issues are classified and tracked, never counted
  as passes. See [test262-roundtrip.md](./docs/test262-roundtrip.md).
- **97.7% pattern recovery across 1,545 transpiler × minifier shapes.**
  Reproduction matrices compile known inputs through real Babel/TypeScript/
  SWC/esbuild/Terser version combinations and verify Wakaru recovers the
  original construct. Current rates per matrix:
  [`scripts/repro/stats.json`](./scripts/repro/stats.json).
- **Fast enough to not think about.** The engine is Rust on SWC with
  parallel module decompilation — a 10 MB production bundle (4,500+ modules)
  unpacks and decompiles in seconds on a laptop. The legacy TypeScript
  implementation needed minutes and could run out of memory on bundles the
  Rust engine handles routinely.

## How it compares

| | **Wakaru** | [webcrack](https://github.com/j4k0xb/webcrack) | [humanify](https://github.com/jehna/humanify) |
|---|---|---|---|
| Focus | bundle splitting + transpiler/minifier decompilation | deobfuscation (obfuscator.io) + unbundling | LLM-based identifier renaming |
| Bundle formats | webpack 4/5, esbuild, Bun, Browserify, SystemJS, AMD/UMD, scope-hoisted ESM | webpack, browserify | via webcrack |
| Transpiler helper recovery | Babel, TypeScript/tslib, SWC (async/await, classes, JSX, enums, …) | partial | — |
| Deobfuscation | ✗ — pair with webcrack (below) | ✓ its specialty | via webcrack |
| Name recovery | source maps + heuristics | — | ✓ its specialty (LLM) |
| Semantic test suite | ✓ Test262 round-trip, 0 failures | — | — |
| Engine | Rust (SWC), parallel | TypeScript (Babel) | TypeScript + LLM |

These tools compose rather than compete. Spot an error in this table? Open an
issue — it should stay fair.

**Obfuscated input?** Wakaru is deliberately not a deobfuscator — heavy
obfuscation (string arrays, control-flow flattening, VM-based protectors) is
a different arms race, and [webcrack](https://github.com/j4k0xb/webcrack) is
the state of the art there. The pipeline that works:

```bash
npx webcrack obfuscated.js -o deobfuscated/   # 1. strip the obfuscation
npx @wakaru/cli deobfuscated/ --unpack -o out/ # 2. recover readable modules
```

**Want better names?** Pair Wakaru's deterministic structure recovery with an
LLM renamer like humanify, or use `--source-map` when a map exists.

## Use cases

- **Security review & bug bounty** — read what a site actually ships instead
  of scrolling one 5 MB line. Split the bundle, find the first-party code,
  audit it as modules.
- **Incident response & malware triage** — unminify a suspicious script into
  something a human can diff and reason about, at `minimal` level so the
  semantics you review are the semantics that ran.
- **Recovering lost source** — the vendor vanished, the laptop died, and all
  that's left is `dist/`. Reconstruct a workable codebase from the bundle
  (and `wakaru extract` recovers originals when source maps were shipped).
- **Debugging third-party SDKs** — turn the vendored blob into readable
  modules so the stack trace points at code you can actually understand.
- **Supply-chain inspection** — see what's inside a dependency's shipped
  bundle rather than trusting the repo it claims to be built from.

## Install

```bash
npm install -g @wakaru/cli@latest
```

Or pre-built binaries from [GitHub Releases](https://github.com/pionxzh/wakaru/releases).
Full CLI documentation: [docs/cli.md](./docs/cli.md).

## Contributing

Every kind of contribution is welcome.

Some areas where help is especially useful:

- Share real-world bundles that Wakaru doesn't handle well
- Report missing helper detection or false positives
- Report semantic or correctness issues

When reporting a bug, please include: the input code, the command you ran, the current output, and what you expected instead.

<details>
<summary>Development setup</summary>

1. Fork the repo and create your branch from `main`
2. Install a stable Rust toolchain
3. Run `cargo test` to verify everything passes
4. Make your changes and add tests

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for full setup notes.

Before submitting a PR:

```bash
cargo test
cargo clippy -- -D warnings
```

This project uses [Conventional Commits](https://www.conventionalcommits.org/). Please mention the issue number in the commit message or PR description.

Docs: [`docs/README.md`](./docs/README.md) is the index; start with [`architecture.md`](./docs/architecture.md).

</details>

## License

[Apache-2.0](./LICENSE)

<sub>Usage of wakaru for attacking targets without prior mutual consent is illegal. End users are responsible for complying with all applicable laws.</sub>
