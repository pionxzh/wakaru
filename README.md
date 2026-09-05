<div align="center">

# Wakaru

**Unpack. Unminify. Understand.**

Wakaru is a JavaScript decompiler. It splits production bundles into modules
and restores readable, modern syntax from minified and transpiled code.

[![CI](https://img.shields.io/github/actions/workflow/status/pionxzh/wakaru/rust-ci.yml?branch=main&label=CI)](https://github.com/pionxzh/wakaru/actions/workflows/rust-ci.yml)
[![npm](https://img.shields.io/npm/v/@wakaru/cli?label=npm)](https://www.npmjs.com/package/@wakaru/cli)
[![Telegram](https://img.shields.io/badge/Telegram-group-blue)](https://t.me/wakarujs)

[**Try it in the playground**](https://wakarujs.com/playground)

</div>

## Quick start

Run without a global install:

```bash
npx wakaru input.js -o output.js       # decompile a file
npx wakaru bundle.js --unpack -o out/  # unpack and decompile a bundle
npx wakaru dist/ --unpack -o out/      # scan a bundle output directory
```

See the [CLI reference](./docs/cli.md) for rewrite levels, source maps, and more options.

### Install

For regular use, install the CLI globally:

```bash
npm install -g wakaru@latest
```

Standalone binaries are available from
[GitHub Releases](https://github.com/pionxzh/wakaru/releases).

## What it does

Wakaru rewrites the JavaScript AST to recover modern syntax, remove recognized
runtime helpers, and split supported bundles into modules.

**Minified Babel output:**

```js
"use strict";Object.defineProperty(exports,"__esModule",{value:!0}),exports.loadProfile=void 0;
var _api=_interopRequireDefault(require("./api"));
function _interopRequireDefault(e){return e&&e.__esModule?e:{default:e}}
function _asyncToGenerator(e){return function(){var t=this,r=arguments;return new Promise(function(n,o){var a=e.apply(t,r);function i(e){c(a,n,o,i,u,"next",e)}function u(e){c(a,n,o,i,u,"throw",e)}i(void 0)})}}
function c(e,t,r,n,o,a,i){try{var u=e[a](i),c=u.value}catch(e){return void r(e)}u.done?t(c):Promise.resolve(c).then(n,o)}
var loadProfile=function(){var e=_asyncToGenerator(function*(e){var t=yield _api.default.fetchUser(e),r=null!=t.name?t.name:"anonymous";return{name:r,avatar:null==t.profile?void 0:t.profile.avatar}});return function(t){return e.apply(this,arguments)}}();exports.loadProfile=loadProfile;
```

**Wakaru output:**

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

The helpers become `async`/`await`, the null checks become `??` and `?.`, and
CommonJS imports and exports become ESM. Some names, such as `e` and `t`, stay
short because the input does not provide their original names.

## What it handles

- **Bundles:** split webpack, esbuild/Bun, Browserify, and Metro output into readable modules.
- **Transpiled code:** recover modern syntax from Babel, TypeScript, and SWC output.
- **Minified code:** expand compact expressions and simplify control flow.

See [supported bundle formats](./docs/cli.md#unpack-bundles-and-chunks) for the full list and format-specific limits.

## Tested like a compiler

We test both behavior and recovery against real compiler and minifier output:

- **62,061 passing Test262 semantic round trips.** [Methodology and baseline](./docs/test262-roundtrip.md).
- **97.4% pattern recovery across 1,858 transpiler × minifier test shapes.** [Per-matrix results](./scripts/repro/stats.json).

## Use cases

- **Security and supply-chain review:** inspect the JavaScript a site or
  dependency ships, with supported bundles split into modules.
- **Debugging third-party SDKs:** follow the code behind a stack trace or
  investigate behavior in a distributed build.
- **Source recovery:** recover readable modules when only build artifacts
  remain, or extract original files embedded in source maps.

## Works with other tools

Wakaru focuses on minifier and transpiler recovery. For supported obfuscation
patterns, a deobfuscator such as [webcrack](https://github.com/j4k0xb/webcrack)
can prepare the input before Wakaru processes it. Heavy control-flow or
VM-based obfuscation needs a dedicated approach.

For AI-assisted identifier naming, see
[humanify](https://github.com/jehna/humanify). Inferred names are reading aids;
source maps are the source of original names when available.

## Use it from an agent

Give your coding agent readable modules to search and inspect. With the
Wakaru skill, it can unpack a bundle and focus on the files relevant to your
question, keeping unrelated code out of context.

Install via skills.sh:

```bash
npx skills add pionxzh/wakaru
```

## In development: package inventory

Package inventory is under development: identify which npm packages and
versions a production bundle contains. It is not part of the current release.

[Share your use case](https://github.com/pionxzh/wakaru/issues/new?template=package_inventory_interest.yml)
to help shape its scope and priority, or contact
[hello@wakarujs.com](mailto:hello@wakarujs.com).

## Contributing

Small fixes, missing recovery patterns, and correctness reports are welcome.
For a bug report, include the input, command, current output, and expected
behavior. A clear issue is useful even without a proposed fix.

See [CONTRIBUTING.md](./CONTRIBUTING.md) for setup and PR guidance, and
[docs/README.md](./docs/README.md) for the development documentation.

## License

[Apache-2.0](./LICENSE)

<sub>Usage of Wakaru for attacking targets without prior mutual consent is illegal. End users are responsible for complying with all applicable laws.</sub>
