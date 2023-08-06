# @unminify-kit/unminify

If you have been working with minified code, you may have noticed that it is not very readable. This is where these transformations come in. They are a set of functions that can be used to modify the code in a way that makes it more readable. The transformations are not limited to just minified code, they can be used to modify any code.

- [@unminify-kit/unminify](#unminify-kitunminify)
    - [`lebab`](#lebab)
    - [`un-boolean`](#un-boolean)
    - [`un-void-0`](#un-void-0)
    - [`un-number-literal`](#un-number-literal)
    - [`un-sequence-expression`](#un-sequence-expression)
    - [`un-variable-merging`](#un-variable-merging)
    - [`un-if-statement`](#un-if-statement)
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

### `un-boolean`

This transformation converts `!0` to `true` and `!1` to `false`.\
Reverse: [babel-plugin-transform-minify-booleans](https://babeljs.io/docs/en/babel-plugin-transform-minify-booleans)

```diff
- !0
+ true

- !1
+ false
```

### `un-void-0`

This transformation converts `void 0` to `undefined`.\
Reverse: [babel-plugin-transform-undefined-to-void](https://babeljs.io/docs/en/babel-plugin-transform-undefined-to-void)

```diff
- if(input === void 0) {}
+ if(input === undefined) {}
```

### `un-number-literal`

This transformation converts minified number literals to their decimal representation.\
Reverse: [babel-plugin-minify-numeric-literals](https://babeljs.io/docs/en/babel-plugin-minify-numeric-literals)


```diff
- 1e3
+ 1000

- -2e4
+ -20000
```

### `un-sequence-expression`

This transformation splits sequence expressions into multiple statements.\
Reverse: [babel-helper-to-multiple-sequence-expressions](https://babeljs.io/docs/en/babel-helper-to-multiple-sequence-expressions)

```diff
- a(), b(), c()
+ a()
+ b()
+ c()

- return a(), b(), c()
+ a()
+ b()
+ return c()
```

### `un-variable-merging`

This transformation splits variable declarations into multiple statements.\
Reverse: [babel-plugin-transform-merge-sibling-variables](https://babeljs.io/docs/en/babel-plugin-transform-merge-sibling-variables)

```diff
- var a = 1, b = 2, c = 3;
+ var a = 1;
+ var b = 2;
+ var c = 3;

- let d = 1, e = 2;
+ let d = 1;
+ let e = 2;

- const f = 1, g = 2;
+ const f = 1;
+ const g = 2;
```

### `un-if-statement`

This transformation unwraps nested ternary expressions into if-else statements.\
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

And this rule will try to do more by adopting `Early Exit` pattern(on statement level).
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

### `un-es6-class`

This transformation will try to build the class definition from the constructor and the prototype.\
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

This transformation removes the `__esModule` flag.

```diff
- Object.defineProperty(exports, "__esModule", { value: true });
```

### `un-strict`

This transformation removes the `"use strict"` directive.

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
