# Wakaru

[![deploy][DeployBadge]][Playground]

Wakaru is the Javascript decompiler for modern frontend. It brings back the original code from a bundled and transpiled source.

Try it out at the [Playground][Playground].

- 🔪📦 Unpacks bundled JavaScript into separated modules from [webpack][webpack] and [browserify][browserify].
- ⛏️📜 Unminifies transpiled code from [Terser][Terser], [Babel][Babel], [SWC][SWC], and [TypeScript][TypeScript].
- ✨📚 Detects and restores downgraded syntaxes (even with helpers!). See the [list](./packages//unminify/README.md#syntax-upgrade).
- 🧪🛡️ All cases are protected by tests. All code is written in TypeScript.

## Examples

### Color function

#### In

<details>
  <summary>Click to expand</summary>

```js
var r = require(6854);
var a = function (e, t) {
  if (!e.startsWith("#")) return e;
  var n = f(e);
  var o = (0, r.Z)(n, 3);
  var a = o[0];
  var s = o[1];
  var c = o[2];
  return "rgba("
    .concat(a, ", ")
    .concat(s, ", ")
    .concat(c, ", ")
    .concat(t, ")");
};
exports.color = a;
```

</details>

#### Out

<details>
  <summary>Click to expand</summary>

```js
export const color = (e, t) => {
  if (!e.startsWith("#")) {
    return e;
  }
  const [a, s, c] = f(e);
  return `rgba(${a}, ${s}, ${c}, ${t})`;
};
```
</details>

### React component

#### In

<details>
  <summary>Click to expand</summary>

```js
var r = require(7462);
var o = require(6854);

var d = function (e) {
  var t = e.children,n = e.className,c = e.visible,f = e.name;
  var p = (0, r.useState)(""), h = (0, o.Z)(p, 2);
  var g = h[0], y = h[1];
  var b = (0, r.useState)(c),v = (0, o.Z)(b, 2);
  var w = v[0],x = v[1];

  return ((0, r.useEffect)(
    function () {
      var e = !0 == c ? "enter" : "leave";
      c && !w && x(!0);
      y("".concat(f, "-").concat(e));

      var n = setTimeout(function () {
        y("".concat(f, "-").concat(e, " ").concat(f, "-").concat(e, "-active"));
        clearTimeout(n);
      }, 1e3);

      return function () {
        clearTimeout(n);
      };
    },
    [c, w]
  ),
  r.createElement("div", { className: "".concat(n, " ").concat(g) }, t));
}
d.displayName = "CssTransition";
```

</details>

#### Out

<details>
  <summary>Click to expand</summary>

```js
import { useState, useEffect } from "module-7462.js";

const CssTransition = (e) => {
  const {
    children,
    className,
    visible,
    name,
  } = e;
  const [g, y] = useState("");
  const [w, x] = useState(visible);

  useEffect(() => {
    const e = visible == true ? "enter" : "leave";
    if (visible && !w) {
      x(true);
    }
    y(`${name}-${e}`);

    const n = setTimeout(() => {
      y(`${name}-${e} ${name}-${e}-active`);
      clearTimeout(n);
    }, 1000);

    return () => {
      clearTimeout(n);
    };
  }, [visible, w]);
  return (
    <div
      className={`${className} ${g}`}
    >
      {children}
    </div>
  )
}
CssTransition.displayName = "CssTransition";
```

</details>

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
[DeployBadge]: https://therealsujitk-vercel-badge.vercel.app/?app=wakaru

## License

[MIT](./LICENSE)
