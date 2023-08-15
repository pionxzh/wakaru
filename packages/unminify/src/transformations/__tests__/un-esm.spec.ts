import transform from '../un-esm'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('require to import',
  `
var foo = require('foo');
var { bar } = require('foo');
var baz1 = require('baz2').baz3;
require('side-effect');
`,
  `
import { baz3 } from "baz2";
import "side-effect";
import foo, { bar } from "foo";
var baz1 = baz3;
`,
)

inlineTest('require with destructuring and property access',
  `
var { bar } = require('baz2').baz2;
var baz1 = require('baz2').baz3;
`,
  `
import { baz2, baz3 } from "baz2";
var { bar } = baz2;
var baz1 = baz3;
`,
)

inlineTest('require with property access and naming conflict',
  `
var { baz } = require('foo').bar;
var bar = 1;
console.log(bar);
`,
  `
import { bar } from "foo";
var { baz } = bar;
var _bar = 1;
console.log(_bar);
`,
)

inlineTest('default export primitive', 'module.exports = 1;', 'export default 1;')
inlineTest('default export object', 'module.exports = { foo: 1 };', 'export default { foo: 1 };')
inlineTest('default export function', 'module.exports = function() {};', 'export default function() {};')
inlineTest('default export function with name', 'module.exports = function bar() {};', 'export default function bar() {};')
inlineTest('default export class', 'module.exports = class {};', 'export default class {};')

inlineTest('default export primitive', 'module.exports.default = 1;', 'export default 1;')
inlineTest('default export object', 'module.exports.default = { foo: 1 };', 'export default { foo: 1 };')
inlineTest('default export function', 'module.exports.default = function() {};', 'export default function() {};')
inlineTest('default export function with name', 'module.exports.default = function bar() {};', 'export default function bar() {};')
inlineTest('default export class', 'module.exports.default = class {};', 'export default class {};')

inlineTest('default export primitive', 'exports.default = 1;', 'export default 1;')
inlineTest('default export object', 'exports.default = { foo: 1 };', 'export default { foo: 1 };')
inlineTest('default export function', 'exports.default = function() {};', 'export default function() {};')
inlineTest('default export function with name', 'exports.default = function bar() {};', 'export default function bar() {};')
inlineTest('default export class', 'exports.default = class {};', 'export default class {};')

inlineTest('named export primitive', 'exports.foo = 1;', 'export const foo = 1;')
inlineTest('named export object', 'exports.foo = { foo: 1 };', 'export const foo = { foo: 1 };')
inlineTest('named export function', 'exports.foo = function() {};', 'export const foo = function() {};')
inlineTest('named export function with name', 'exports.foo = function bar() {};', 'export const foo = function bar() {};')
inlineTest('named export class', 'exports.foo = class {};', 'export const foo = class {};')

inlineTest('named export primitive', 'module.exports.foo = 1;', 'export const foo = 1;')
inlineTest('named export object', 'module.exports.foo = { foo: 1 };', 'export const foo = { foo: 1 };')
inlineTest('named export function', 'module.exports.foo = function() {};', 'export const foo = function() {};')
inlineTest('named export function with name', 'module.exports.foo = function bar() {};', 'export const foo = function bar() {};')
inlineTest('named export class', 'module.exports.foo = class {};', 'export const foo = class {};')

// TODO: a new rule to convert export const foo = function() {} to export function foo() {}

inlineTest('named exports strategy',
  `
function same() {}
module.exports.same = same;

class StillSame {}
exports.Another = StillSame;
`,
  `
function same() {}
export { same };

class StillSame {}
export const Another = StillSame;
`,
)

inlineTest('named exports strategy 2',
  `
module.exports.foo = foo
exports.bar = bar
`,
  `
export { foo };
export { bar };
`,
)

// This is a quite common pattern in some bundlers
// They will initialize the exports object first,
// then create that object and put it back to module.exports
inlineTest('duplicate exports',
  `
module.exports.foo = null;
module.exports.foo = 2;
`,
  `
export const foo = 2;
`,
)

inlineTest('duplicate default exports',
  `
module.exports = 1;
module.exports = 2;
`,
  `
export default 2;
`,
)

inlineTest('duplicate default exports 2',
  `
module.exports = 1;
module.exports.default = 2;
`,
  `
export default 2;
`,
)

inlineTest('variable declaration with exports',
  `
var foo = exports.foo = 1;
var bar = exports.baz = 2;
var dan = module.exports.dan = 3;
var qux = module.exports.quz = 4;
`,
  `
export var foo = 1;
var bar = baz;
export var baz = 2;
export var dan = 3;
var qux = quz;
export var quz = 4;
`,
)

inlineTest('variable declaration with default exports',
  `
var foo = exports.default = 1;
`,
  `
var foo = 1;
export default foo;
`,
)

inlineTest('variable declaration with default exports 2',
  `
var foo = module.exports.default = 1;
`,
  `
var foo = 1;
export default foo;
`,
)

inlineTest('export with name conflict',
  `
var foo = 1;
console.log(foo);
exports.foo = 2;

const bar = 2;
const _bar = 3;
console.log(bar, _bar);
module.exports.bar = 4;
`,
  `
var _foo = 1;
console.log(_foo);
export const foo = 2;

const _bar_1 = 2;
const _bar = 3;
console.log(_bar_1, _bar);
export const bar = 4;
`,
)

inlineTest('mixed exports (which is actually not correct)',
  `
module.exports = obj;
module.exports.foo = 1;
`,
  `
export default obj;
export const foo = 1;
`,
)

inlineTest('should not transform these invalid export',
  `
exports = 1;
exports = function() {};
module.exports += 1;
module["exports"] = 1;
`,
  `
exports = 1;
exports = function() {};
module.exports += 1;
module["exports"] = 1;
`,
)
