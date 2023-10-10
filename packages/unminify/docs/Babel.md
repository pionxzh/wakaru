# Babel

This is the tracking list of support status of the reversion of [babel-preset-minify](https://babeljs.io/docs/babel-preset-minify) in Babel.

- ~~Strike through~~ means the option is invalid, or not possible to be reversed, or out of the scope of this project.


---

- [X] transform-minify-booleans
  - `un-boolean`
- [ ] minify-builtins
  - TODO: priority `medium`.
- [X] ~~transform-inline-consecutive-adds~~
- [X] ~~minify-dead-code-elimination~~
- [X] ~~minify-constant-folding~~
- [X] minify-flip-comparisons
  - `un-flip-comparisons`
- [X] minify-guarded-expressions
  - `un-conditionals`
- [X] minify-infinity
  - `un-infinity`
- [X] ~~minify-mangle-names~~
- [X] transform-member-expression-literals
  - `un-bracket-notation`
- [X] transform-merge-sibling-variables
  - `un-variable-merging`
- [X] minify-numeric-literals
  - `un-numeric-literal`
- [X] ~~transform-property-literals~~
- [X] ~~transform-regexp-constructors~~
- [X] ~~transform-remove-console~~
- [X] ~~transform-remove-debugger~~
- [X] ~~transform-remove-undefined~~
- [X] ~~minify-replace~~
- [ ] minify-simplify
  - `un-conditionals`
- [X] ~~transform-simplify-comparison-operators~~
- [X] minify-type-constructors
  - `un-type-constructors`
- [X] transform-undefined-to-void
  - `un-undefined`
