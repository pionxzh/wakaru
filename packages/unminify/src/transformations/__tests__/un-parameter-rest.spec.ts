import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../un-parameter-rest'

const inlineTest = defineInlineTest(transform)

inlineTest('does not replace arguments outside a function',
  `
console.log(arguments);
`,
  `
console.log(arguments);
`,
)

inlineTest('replaces arguments in function declaration',
  `
function foo() {
  console.log(arguments);
}
`,
  `
function foo(...args) {
  console.log(args);
}
`,
)

inlineTest('replaces arguments in function expression',
  `
var foo = function() {
  console.log(arguments);
}
`,
  `
var foo = function(...args) {
  console.log(args);
}
`,
)

inlineTest('replaces arguments in class method',
  `
class Foo {
  bar() {
    console.log(arguments);
  }
}
`,
  `
class Foo {
  bar(...args) {
    console.log(args);
  }
}
`,
)

inlineTest('does not replace arguments in arrow function',
  `
var foo = () => console.log(arguments);
`,
  `
var foo = () => console.log(arguments);
`,
)

inlineTest('replaces arguments in nested arrow function',
  `
function foo() {
  var bar = () => console.log(arguments);
}
`,
  `
function foo(...args) {
  var bar = () => console.log(args);
}
`,
)

inlineTest('does not replace arguments when args variable already exists',
  `
function foo() {
  var args = [];
  console.log(arguments);
}
`,
  `
function foo() {
  var args = [];
  console.log(arguments);
}
`,
)

inlineTest('does not replace arguments when args variable exists in parent scope',
  `
var args = [];
function foo() {
  console.log(args, arguments);
}
`,
  `
var args = [];
function foo() {
  console.log(args, arguments);
}
`,
)

inlineTest('does not replace arguments when args variable exists in parent function param',
  `
function parent(args) {
  function foo() {
    console.log(args, arguments);
  }
}
`,
  `
function parent(args) {
  function foo() {
    console.log(args, arguments);
  }
}
`,
)

inlineTest('does not replace arguments when args variable exists in child block scope that uses arguments',
  `
function foo() {
  if (true) {
    const args = 0;
    console.log(arguments);
  }
}

function foo2() {
  for (var _len3 = arguments.length, args = new Array(_len3), _key3 = 0; _key3 < _len3; _key3++) {
    args[_key3] = arguments[_key3];
  }
  args.pop();
  foo.apply(void 0, args);
}
`,
  `
function foo() {
  if (true) {
    const args = 0;
    console.log(arguments);
  }
}

function foo2() {
  for (var _len3 = arguments.length, args = new Array(_len3), _key3 = 0; _key3 < _len3; _key3++) {
    args[_key3] = arguments[_key3];
  }
  args.pop();
  foo.apply(void 0, args);
}
`,
)

inlineTest('does not replace arguments in function declaration with existing formal params',
  `
function foo(a, b ,c) {
  console.log(arguments);
}
`,
  `
function foo(a, b ,c) {
  console.log(arguments);
}
`,
)

inlineTest('does not add ...args to function that does not use arguments',
  `
function foo() {
  console.log(a, b, c);
}
`,
  `
function foo() {
  console.log(a, b, c);
}
`,
)
