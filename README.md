# Wakaru

[![deploy][DeployBadge]][Playground]

Wakaru is the Javascript decompiler for modern frontend. It brings back the original code from a bundled and transpiled source.

<!-- Try it out at the [Playground][Playground]. -->

- 🔪📦 Unpacks bundled JavaScript into separated modules from [webpack][webpack] and [browserify][browserify].
- ⛏️📜 Unminifies transpiled code from [Terser][Terser], [Babel][Babel], [SWC][SWC], and [TypeScript][TypeScript].
- ✨📚 Detects and restores downgraded syntaxes (even with helpers!). See the [list](./packages//unminify/README.md#syntax-upgrade).
- 🧪🛡️ All cases are protected by tests. All code is written in TypeScript.

## Demo

See [live demo][Demo] for detailed examples.

## Features

### Unminify

Converts transpiled code back to its readable form and restores downgraded syntaxes.

Supports the following transpilers:
  - [Terser][Terser]
  - [Babel][Babel]
  - [SWC][SWC]
  - [TypeScript][TypeScript]

See [Unminify Documentation](./packages/unminify/README.md) for the full list of supported rules.

### Unpacker

Converts bundled JavaScript into separated modules

Supports the following bundlers:
  - [webpack][webpack]
  - [browserify][browserify]

## Try it out

Test the tool and see it in action at [Playground][Playground].

## 🖥 Command Line Interface

🚧🚧🚧 Under construction.

## Motivation

Over the course of developing plugins for io games, the need to understand game logic behind minified code became a recurring challenge. Existing tools often failed to produce readable code, and were often limited to a single bundler or transpiler. This repo was created to address these issues, and provide a single tool capable of handling a wide variety of bundlers and transpilers.

## Legal Disclaimer

Usage of `wakaru` for attacking targets without prior mutual consent is illegal. It is the end user's responsibility to obey all applicable local, state and federal laws. Developers assume no liability and are not responsible for any misuse or damage caused by this program.

[TypeScript]: https://www.typescriptlang.org/
[browserify]: http://browserify.org/
[webpack]: https://webpack.js.org/
[Terser]: https://terser.org/
[Babel]: https://babeljs.io/
[SWC]: https://swc.rs/
[Playground]: https://wakaru.vercel.app/
[Demo]: https://wakaru.vercel.app/#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
[DeployBadge]: https://therealsujitk-vercel-badge.vercel.app/?app=wakaru

## License

[MIT](./LICENSE)
