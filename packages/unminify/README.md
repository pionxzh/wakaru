# @unminify-kit/unminify

This package offers a comprehensive set of transformation rules designed to unminify and enhance the readability of code.

It covered most of patterns that are used by the following tools:
- [Terser](https://terser.org/) (Check the [Progress](./docs/Terser.md))
- [Babel](https://babeljs.io/) (Check the [Progress](./docs/Babel.md))
- [SWC](https://swc.rs/) (Check the [Progress](./docs/SWC.md))
- [TypeScript](https://www.typescriptlang.org/)

## Table of Contents

- [Readability](#readability)
  - [`un-boolean`](#un-boolean)
  - [`un-undefined`](#un-undefined)
  - [`un-infinity`](#un-infinity)
  - [`un-numeric-literal`](#un-numeric-literal)
  - [`un-sequence-expression`](#un-sequence-expression)
  - [`un-variable-merging`](#un-variable-merging)
  - [`un-bracket-notation`](#un-bracket-notation)
  - [`un-while-loop`](#un-while-loop)
  - [`un-flip-operator`](#un-flip-operator)
  - [`un-conditionals`](#un-conditionals)
  - [`un-type-constructor` (Unsafe)](#un-type-constructor-unsafe)
  - [`un-builtin-prototype`](#un-builtin-prototype)
  - [`un-iife`](#un-iife)
- [Syntax Upgrade](#syntax-upgrade)
  - [`un-esm` (Unsafe)](#un-esm-unsafe)
  - [`un-template-literal`](#un-template-literal)
  - [`un-optional-chaining` (Experimental)](#un-optional-chaining-experimental)
  - [`un-nullish-coalescing` (Experimental)](#un-nullish-coalescing-experimental)
  - [`un-es6-class`](#un-es6-class)
  - [`un-async-await` (Experimental)](#un-async-await-experimental)
- [Clean Up](#clean-up)
  - [`un-esmodule-flag`](#un-esmodule-flag)
  - [`un-use-strict`](#un-use-strict)
- [Style](#style)
  - [`prettier`](#prettier)
- [Extra](#extra)
  - [`lebab`](#lebab)
- [TODO](#todo)

## Readability

### `un-boolean`

Converts minified `boolean` to simple `true`/`false`.

```diff
- !0
+ true

- !1
+ false
```

### `un-undefined`

Converts `void 0` to `undefined`.

```diff
- if(input === void 0) {}
+ if(input === undefined) {}
```

### `un-infinity`
Converts `1 / 0` to `Infinity`.


```diff
- 1 / 0
+ Infinity
- -1 / 0
+ -Infinity
```

### `un-numeric-literal`
Converts numeric literal to its decimal representation.\
A comment will be added to indicate the original value.


```diff
- 1e3
+ 1000 /* 1e3 */

- 0b101010
+ 42 /* 0b101010 */

- 0x123
+ 291 /* 0x123 */
```

### `un-sequence-expression`

Separate sequence expressions into multiple statements.

```diff
- a(), b(), c()
+ a()
+ b()
+ c()

- return a(), b()
+ a()
+ return b()

- while (a(), b(), c++ > 0) {}
+ a()
+ b()
+ while (c++ > 0) {}
```

### `un-variable-merging`

Separate variable declarators into multiple statements.

```diff
- var a = 1, b = true, c = func(d):
+ var a = 1;
+ var b = true;
+ var c = func(d);
```

Separate variable declarators that are not used in for statements.

```diff
- for (var i = 0, j = 0, k = 0; j < 10; k++) {}
+ var i = 0
+ for (var j = 0, k = 0; j < 10; k++) {}
```

### `un-bracket-notation`

Simplify bracket notation.

```diff
- obj['prop']
+ obj.prop

- obj['var']
+ obj['var']
```

### `un-while-loop`

Converts for loop without init and update to while loop.

```diff
- for (;;) {}
+ while (true) {}

- for (; i < 10;) {
-  console.log(i);
- }
+ while (i < 10) {
+   console.log(i);
+ }
```

### `un-flip-operator`

Flips comparisons that are in the form of "literal comes first" to "literal comes second".

```diff

```diff
- if ("dark" === theme) {}
+ if (theme === "dark") {}

- while (10 < count) {}
+ while (count > 10) {}
```

### `un-conditionals`

Unwraps nested ternary expressions and binary expression into if-else statements or switch statements.

#### If-Else

```diff
- a ? b() : c ? d() : e()
+ if (a) {
+   b();
+ } else if (c) {
+   d();
+ } else {
+   e();
+ }
```

This rule will try to adopting `Early Exit` pattern if possible.

```diff
function fn () {
-   return a ? b() : c ? d() : e()
+   if (a) {
+     return b();
+   }
+   if (c) {
+     return d();
+   }
+   return e();
}
```

#### Switch

```diff
- foo == 'bar' ? bar() : foo == 'baz' ? baz() : foo == 'qux' || foo == 'quux' ? qux() : quux()
+ switch (foo) {
+   case 'bar':
+     bar()
+     break
+   case 'baz':
+     baz()
+     break
+   case 'qux':
+   case 'quux':
+     qux()
+     break
+   default:
+     quux()
+ }
```

### `un-type-constructor` (Unsafe)

Restore type constructors from minified code.

```diff
- +x;
+ Number(x);

- x + "";
+ String(x);

- [,,,];
+ Array(3);
```

Unsafe:
- BigInt: `+1n` will throw `TypeError`
- Symbol: `Symbol('foo') + ""` will throw `TypeError`

### `un-builtin-prototype`

Convert function calls on instances of built-in objects to equivalent calls on their prototypes.

```diff
- [].splice.apply(a, [1, 2, b, c]);
+ Array.prototype.splice.apply(a, [1, 2, b, c]);

- (function() {}).call.apply(console.log, console, ["foo"]),
+ Function.prototype.call.apply(console.log, console, ["foo"]),

- 0..toFixed.call(Math.PI, 2);
+ Number.prototype.toFixed.call(Math.PI, 2);

- ({}).hasOwnProperty.call(d, "foo");
+ Object.prototype.hasOwnProperty.call(d, "foo");

- /t/.test.call(/foo/, "bar");
+ RegExp.prototype.test.call(/foo/, "bar");

- "".indexOf.call(e, "bar");
+ String.prototype.indexOf.call(e, "bar");
```

### `un-iife`

Improve the readability of code inside IIFE. Useful for short code snippets / userscripts.

Rename the parameters and move the passed-in arguments to the top.

```diff
- (function(i, s, o, g, r, a, m) {
-   i['GoogleAnalyticsObject'] = r;
-   i[r].l = 1 * new Date();
-   a = s.createElement(o);
-   m = s.getElementsByTagName(o)[0];
-   a.async = 1;
-   a.src = g;
-   m.parentNode.insertBefore(a, m);
- })(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
+ (function(window, document, a, m) {
+   const o = 'script';
+   const g = 'https://www.google-analytics.com/analytics.js';
+   const r = 'ga';
+   window['GoogleAnalyticsObject'] = r;
+   window[r].l = 1 * new Date();
+   a = document.createElement(o);
+   m = document.getElementsByTagName(o)[0];
+   a.async = 1;
+   a.src = g;
+   m.parentNode.insertBefore(a, m);
+ })(window, document);
```

## Syntax Upgrade

### `un-esm` (Unsafe)

Converts CommonJS's `require` and `module.exports` to ES6's `import` and `export`.

```diff
- const foo = require('foo')
- var { bar } = require('bar')
- var baz = require('baz').baz
- require('side-effect')
+ import foo from 'foo'
+ import { bar } from 'bar'
+ import { baz } from 'baz'
+ import 'side-effect'
```

```diff
- module.exports.foo = 1
- exports.bar = bar
+ export const foo = 1
+ export { bar }
```

```diff
- module.exports.default = foo
+ export default foo
```

Note: Please aware that CJS and ESM are not fully compatible, and this transformation is not perfect. We have a comprehensive test suite to ensure the correctness of this transformation, but there are still some edge cases that are not covered. Feel free to open an issue if you find any bugs.

Limitations:
- `require(dynamic)` is not supported as ESM does not support dynamic imports. Convert it to `await import()` is not appropriate as it require the whole execution context to be async.
- Some packages require `import * as name from 'package'` instead of `import name from 'package'`. We cannot detect this automatically, so you might need to fix it manually.
- Currently, it won't aware the exports format of other files generated by our `unpacker`. PRs are welcome.

### `un-template-literal`

Restore template literal syntax from string concatenation.

```diff
- "the ".concat(first, " take the ").concat(second, " and ").concat(third);
+ `the ${first} take the ${second} and ${third}`
```

### `un-optional-chaining` (Experimental)

Restore optional chaining syntax.\
Support output from **TypeScript**, **Babel** and **SWC**.

```diff
- (_foo = foo) === null || _foo === void 0 ? void 0 : _foo.bar;
+ foo?.bar;

- (_foo = foo) === null || _foo === void 0 ? void 0 : (_foo_bar = _foo.bar) === null || _foo_bar === void 0 ? void 0 : _foo_bar.baz;
+ foo?.bar?.baz;
```

### `un-nullish-coalescing` (Experimental)

Restore nullish coalescing syntax.\
Support output from **TypeScript**, **Babel** and **SWC**.

```diff
- (_ref = foo !== null && foo !== void 0 ? foo : bar) !== null && _ref !== void 0 ? _ref : "quz";
+ foo ?? bar ?? "quz";
```

### `un-es6-class`

Restore `Class` definition from the constructor and the prototype.\
Currently, this transformation only supports output from **TypeScript**.

Supported features:
- constructor
- instance properties
- instance methods
- static methods
- static properties
- getters and setters
- async method (has limitations from [`un-async-await`](#un-async-await-experimental))

Unsupported features:
- inheritance
- decorators
- private flag(#)

```diff
- var Foo = (function() {
-   function t(name) {
-     this.name = name;
-     this.age = 18;
-   }
-   t.prototype.hi = function logger() {
-     console.log("Hello", this.name);
-   };
-   t.staticMethod = function staticMethod() {
-     console.log("static");
-   };
-   t.instance = new t("foo");
-   return t;
- })();
+ class Foo {
+   constructor(name) {
+     this.name = name;
+     this.age = 18;
+   }
+   hi() {
+     console.log("Hello", this.name);
+   }
+   static staticMethod() {
+     console.log("static");
+   }
+   static instance = new Foo("foo");
+ }
```

### `un-async-await` (Experimental)

Restore async/await from helper `__awaiter` and `__generator`.\
Currently, this transformation only supports output from **TypeScript**.

And it does not handled control flow properly, as it needs graph analysis.

Please aware there are tons of edge cases that are not covered by this rule.

```diff
-function func() {
-  return __awaiter(this, void 0, void 0, function () {
-    var result, json;
-    return __generator(this, function (_a) {
-      switch (_a.label) {
-        case 0:
-          console.log('Before sleep');
-          return [4 /*yield*/, sleep(1000)];
-        case 1:
-          _a.sent();
-          return [4 /*yield*/, fetch('')];
-        case 2:
-          result = _a.sent();
-          return [4 /*yield*/, result.json()];
-        case 3:
-          json = _a.sent();
-          return [2 /*return*/, json];
-      }
-    });
-  });
-}
+async function func() {
+  var result, json;
+  console.log('Before sleep');
+  await sleep(1000);
+  result = await fetch('');
+  json = await result.json();
+  return json;
+}
```

## Clean Up

### `un-esmodule-flag`

Removes the `__esModule` flag from the module.

```diff
- Object.defineProperty(exports, "__esModule", { value: true });
```

### `un-use-strict`

Removes the `"use strict"` directive.

```diff
- "use strict";
```

## Style

### `prettier`

This transformation formats the code with [prettier](https://prettier.io/), typically applied after all other transformations.

## Extra

### `lebab`

> Lebab transpiles your ES5 code to ES6/ES7. It does exactly the opposite of what Babel does.

We use [lebab](https://github.com/lebab/lebab) as a base to unminify the code.\
By utilizing lebab, we can save the repetitive work of writing the transformations ourselves.

## TODO

- [ ] Convert `React.createElement` to JSX.
- [ ] Convert
- [ ] Improve comment retention.
- [ ] Address syntax downgrades from tools like `TypeScript`, `Babel` and `SWC`.
- [ ] `un-string-literal` to decode printable unicode
- [ ] [Terser loops](https://github.com/terser/terser/blob/27c0a3b47b429c605e2243df86044fc00815060f/test/compress/loops.js#L217) contains several useful patterns
