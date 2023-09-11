# Unminify Kit

🔪📦 Reverse-engineer bundled JavaScript code and bring it back to a human-friendly format.

[Try it out](https://unminify.zeabur.app/)

## 🌟 Features

### Unpacker

Converts bundled JavaScript into separate modules

Supports the following bundlers:
  - [webpack](https://webpack.js.org/)
  - [browserify](http://browserify.org/)

### Unminify

Converts transpiled code back to its readable form and restores downgraded syntaxes.

Supports the following transpilers:
  - [Terser](https://terser.org/)
  - [Babel](https://babeljs.io/)
  - [SWC](https://swc.rs/)
  - [TypeScript](https://www.typescriptlang.org/)

See [Unminify Documentation](./packages/unminify/README.md) for the full list of supported rules.

## 🕹 Try it out

Test the tool and see it in action: [Playground](https://unminify.zeabur.app/)

## 🖥 Command Line Interface

🚧 Under construction

```bash
npx @unminify-kit/core [options] <file>
```


## Motivation

Over the course of developing plugins for io games, the need to understand game logic behind minified code became a recurring challenge. Existing tools often failed to produce readable code, and were often limited to a single bundler or transpiler. Unminify Kit was created to address these issues, and provide a single tool capable of handling a wide variety of bundlers and transpilers.

## Legal Disclaimer

Usage of unminify-kit for attacking targets without prior mutual consent is illegal. It is the end user's responsibility to obey all applicable local, state and federal laws. Developers assume no liability and are not responsible for any misuse or damage caused by this program.
