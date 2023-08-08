# @unminify-kit/unminify

If you have been working with minified code, you may have noticed that it is not very readable. This is where these transformations come in. They are a set of functions that can be used to modify the code in a way that makes it more readable. The transformations are not limited to just minified code, they can be used to modify any code.

- [@unminify-kit/unminify](#unminify-kitunminify)
    - [`lebab`](#lebab)
  - [Readability](#readability)
    - [`un-boolean`](#un-boolean)
    - [`un-void-0`](#un-void-0)
    - [`un-number-literal`](#un-number-literal)
    - [`un-sequence-expression`](#un-sequence-expression)
    - [`un-variable-merging`](#un-variable-merging)
    - [`un-flip-operator`](#un-flip-operator)
    - [`un-if-statement`](#un-if-statement)
    - [`un-switch-statement`](#un-switch-statement)
  - [Syntax Upgrade](#syntax-upgrade)
    - [`un-template-literal`](#un-template-literal)
    - [`un-es6-class`](#un-es6-class)
  - [Clean Up](#clean-up)
    - [`un-es-helper`](#un-es-helper)
    - [`un-strict`](#un-strict)
  - [Style](#style)
    - [`function-to-arrow`](#function-to-arrow)
    - [`prettier`](#prettier)

### `lebab`

We use [lebab](https://github.com/lebab/lebab) as a base to unminify the code.\
With the help of lebab, we can save the work of writing a lot of rules.

## Readability

### `un-boolean`

Transform minified `boolean` to their simpler forms.\
Reverse: [babel-plugin-transform-minify-booleans](https://babeljs.io/docs/en/babel-plugin-transform-minify-booleans)

```diff
- !0
+ true

- !1
+ false
```

### `un-void-0`

Transform `void 0` to `undefined`.\
Reverse: [babel-plugin-transform-undefined-to-void](https://babeljs.io/docs/en/babel-plugin-transform-undefined-to-void)

```diff
- if(input === void 0) {}
+ if(input === undefined) {}
```

### `un-number-literal`
Transform number literal to its decimal representation.\
Reverse: [babel-plugin-minify-numeric-literals](https://babeljs.io/docs/en/babel-plugin-minify-numeric-literals)


```diff
- 1e3
+ 1000

- 0b101010
+ 42

- 0x123
+ 291
```

### `un-sequence-expression`

Separate sequence expressions into multiple statements.\
Reverse: [babel-helper-to-multiple-sequence-expressions](https://babeljs.io/docs/en/babel-helper-to-multiple-sequence-expressions)

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
Reverse: [babel-plugin-transform-merge-sibling-variables](https://babeljs.io/docs/en/babel-plugin-transform-merge-sibling-variables)

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

### `un-flip-operator`

Flips comparisons that are in the form of "literal comes first" to "literal comes second".\
Reverse: [babel-plugin-minify-flip-comparisons](https://babeljs.io/docs/en/babel-plugin-minify-flip-comparisons)

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

Reverse: [babel-plugin-minify-guarded-expressions](https://babeljs.io/docs/en/babel-plugin-minify-guarded-expressions)

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

### `un-template-literal`

Restore template literal syntax from string concatenation.

```diff
- "the ".concat(first, " take the ").concat(second, " and ").concat(third);
+ `the ${first} take the ${second} and ${third}`
```

### `un-es6-class`

Restore `Class` definition from the constructor and the prototype.\
Currently, this transformation only supports output from **TypeScript**.

Reverse: `Typescript`'s `Class` transpilation

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

## Clean Up

### `un-es-helper`

Removes the `__esModule` flag from the module.

```diff
- Object.defineProperty(exports, "__esModule", { value: true });
```

### `un-strict`

Removes the `"use strict"` directive.

```diff
- "use strict";
```

## Style

### `function-to-arrow`

This transformation converts a function declaration to an arrow function(if possible).

```diff
- function foo() {
-   return 1;
- }
+ const foo = () => {
+   return 1;
+ }
```

### `prettier`

This transformation formats the code with [prettier](https://prettier.io/).
We usually use this rule to format the code after all the other transformations.
