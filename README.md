# Unminify Kit

🔪📦 An collection of jscodeshift rules to unminify your code.

## Introduction

The ultimate goal of this project is to improve the readability of the minified code.\
This is useful for you to reverse engineer without the source code.

It provides a set of rules to reverse the minification process from:
- **Most** of [babel-preset-minify](https://babeljs.io/docs/babel-preset-minify)
- **Some** of [terser](https://terser.org)
- **Some** of transpilation from [Typescript](https://www.typescriptlang.org/)
- ... and more

## Why?

I have intermittently worked on plugins for io games for a long time.\
So reading through minified code to understand the game logic is a common task for me.

I have tried many tools to unminify the code, but none of them can meet my needs.\
Most of them cannot even beautify the code correctly. So I decided to build my own one.


## Features

-

## Installation

```bash
npm install @unminify-kit/core
```

## Command Line

```bash
npx @unminify-kit/core [input] [options]
```

## Legal Disclaimer

Usage of unminify-kit for attacking targets without prior mutual consent is illegal. It's the end user's responsibility to obey all applicable local, state and federal laws. Developers assume no liability and are not responsible for any misuse or damage caused by this program.

## Similar Projects

- [lebab](https://github.com/lebab/lebab)
- [retidy](https://github.com/Xmader/retidy)
- [unminify](https://github.com/shapesecurity/unminify)
- [5to6-codemod](https://github.com/5to6/5to6-codemod)

- [debundle](https://github.com/1egoman/debundle)
- [de4js](https://github.com/lelinhtinh/de4js)
- [webpack-unpack](https://github.com/goto-bus-stop/webpack-unpack)
