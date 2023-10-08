# Wakaru

[![deploy][DeployBadge]][Playground]

Wakaru is the Javascript decompiler for modern frontend. It brings back the original code from a bundled and transpiled source.

Try it out at our [Playground][Playground].

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

Test the tool and see it in action at [Playground].[Playground]

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
[DeployBadge]: https://therealsujitk-vercel-badge.vercel.app/?app=wakaru
