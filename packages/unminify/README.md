# @unminify-kit/unminify

This package offers a comprehensive set of transformation rules designed to unminify and enhance the readability of code.
- Rules
  - [Readability](#readability)
    - [`un-boolean`](#un-boolean)
    - [`un-undefined`](#un-undefined)
    - [`un-infinity`](#un-infinity)
    - [`un-numeric-literal`](#un-numeric-literal)
    - [`un-sequence-expression`](#un-sequence-expression)
    - [`un-variable-merging`](#un-variable-merging)
    - [`un-bracket-notation`](#un-bracket-notation)
    - [`un-flip-operator`](#un-flip-operator)
    - [`un-if-statement`](#un-if-statement)
    - [`un-switch-statement`](#un-switch-statement)
  - [Syntax Upgrade](#syntax-upgrade)
    - [`un-template-literal`](#un-template-literal)
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

Converts minified `boolean` to simple `true`/`false`.\
Reverse: [babel-plugin-transform-minify-booleans](https://babeljs.io/docs/babel-plugin-transform-minify-booleans)

```diff
- !0
+ true

- !1
+ false
```

### `un-undefined`

Converts `void 0` to `undefined`.\
Reverse: [babel-plugin-transform-undefined-to-void](https://babeljs.io/docs/babel-plugin-transform-undefined-to-void)

```diff
- if(input === void 0) {}
+ if(input === undefined) {}
```

### `un-infinity`
Converts `1 / 0` to `Infinity`.\
Reverse: [babel-plugin-minify-infinity](https://babeljs.io/docs/babel-plugin-minify-infinity)


```diff
- 1 / 0
+ Infinity
- -1 / 0
+ -Infinity
```

### `un-numeric-literal`
Converts numeric literal to its decimal representation.\
A comment will be added to indicate the original value.\
Reverse: [babel-plugin-minify-numeric-literals](https://babeljs.io/docs/babel-plugin-minify-numeric-literals)


```diff
- 1e3
+ 1000 /* 1e3 */

- 0b101010
+ 42 /* 0b101010 */

- 0x123
+ 291 /* 0x123 */
```

### `un-sequence-expression`

Separate sequence expressions into multiple statements.\
Reverse: [babel-helper-to-multiple-sequence-expressions](https://babeljs.io/docs/babel-helper-to-multiple-sequence-expressions)

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

Separate variable declarators into multiple statements.\
Reverse: [babel-plugin-transform-merge-sibling-variables](https://babeljs.io/docs/babel-plugin-transform-merge-sibling-variables)

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

Converts bracket notation to dot notation.\
Reverse: [babel-plugin-transform-member-expression-literals](https://babeljs.io/docs/babel-plugin-transform-member-expression-literals)

```diff
- obj['prop']
+ obj.prop

- obj['var']
+ obj['var']
```

### `un-while-loop`

Converts `for(;;)` to `while(true)`.

```diff
- for (;;) {}
+ while (true) {}
```

### `un-flip-operator`

Flips comparisons that are in the form of "literal comes first" to "literal comes second".\
Reverse: [babel-plugin-minify-flip-comparisons](https://babeljs.io/docs/babel-plugin-minify-flip-comparisons)

```diff

```diff
- if ("dark" === theme) {}
+ if (theme === "dark") {}

- while (10 < count) {}
+ while (count > 10) {}
```

### `un-if-statement`

Unwraps nested ternary expressions into if-else statements.\
Conditionally returns early if possible.

Reverse: [babel-plugin-minify-guarded-expressions](https://babeljs.io/docs/babel-plugin-minify-guarded-expressions)

```diff
- a ? b() : c ? d() : e()
+ if(a) {
+   b();
+ } else if(c) {
+   d();
+ } else {
+   e();
+ }
```

This rule will try to do more by adopting `Early Exit` pattern (on statement level).\
// TODO
```diff
- a ? b() : c ? d() : e()
+ if(a) {
+   b();
+ }
+ if(c) {
+   d();
+ }
+ e();
```

### `un-switch-statement`

Unwraps nested ternary expressions into switch statement.

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

## Syntax Upgrade

### `un-esm`

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

### `un-template-literal`

Restore template literal syntax from string concatenation.

```diff
- "the ".concat(first, " take the ").concat(second, " and ").concat(third);
+ `the ${first} take the ${second} and ${third}`
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

And it does not handled control flow properly, as it needs control glow / graph analysis.

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
- [ ] `un-optional-chaining`
- [ ] `un-nullish-coalescing`
- [ ] `un-string-literal` to decode printable unicode
- [ ] [Terser loops](https://github.com/terser/terser/blob/27c0a3b47b429c605e2243df86044fc00815060f/test/compress/loops.js#L217) contains several useful patterns
