import { defineInlineTest, defineInlineTestWithOptions } from '@unminify-kit/test-utils'
import transform from '../un-esm'

const inlineTest = defineInlineTest(transform)
const inlineTestOpt = defineInlineTestWithOptions(transform)

inlineTest('imports will be collected, merged and dedupe',
  `
import foo from 'foo';
import bar from 'foo';
import { baz } from 'foo';
import { baz as baz1 } from 'foo';
`,
  `
import foo, { baz, baz as baz1 } from "foo";
import bar from "foo";
`,
)

inlineTest('require to import',
  `
var foo = require('foo');
var { bar } = require('foo');
var baz = require('baz').default;
var baz1 = require('baz2').baz3;
require('side-effect');
`,
  `
import foo, { bar } from "foo";
import baz from "baz";
import { baz3 as baz1 } from "baz2";
import "side-effect";
`,
)

inlineTest('default import',
  `
var foo = require('bar');
var baz = require('baz').default;
`,
  `
import foo from "bar";
import baz from "baz";
`,
)

inlineTest('named import',
  `
var foo = require('baz').baz;
var bar = require('baz').baz;
var baz = require('baz')['baz'];
`,
  `
import { baz as foo, baz as bar, baz } from "baz";
`,
)

inlineTest('named import #2',
  `
var { bar, baz: foo } = require('baz');
var { box } = require('box').default;
`,
  `
import { bar, baz as foo } from "baz";
import { box } from "box";
`,
)

inlineTest('namespace import',
  `
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _baz = _interopRequireWildcard(require("baz"));
`,
  `
import * as _baz from "baz";
`,
)

inlineTest('namespace import #2',
  `
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");

var _foo = require("foo");
_foo = _interopRequireWildcard(_foo);

var _bar = require("bar");
_source = _interopRequireWildcard(_bar);

var _baz = require("baz");
_another = _interopRequireWildcard(_baz);
`,
  `
import * as _foo from "foo";
import * as _source from "bar";
import * as _another from "baz";
`,
)

inlineTest('namespace import #3',
  `
import _source$es6Default from "source";
import _another$es6Default from "another";

var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
var _source = _interopRequireWildcard(_source$es6Default);
_source;

var _another = _interopRequireWildcard(_another$es6Default);
_another$es6Default;
`,
  `
import * as _source from "source";
import * as _another from "another";
_source;

_another;
`,
)

inlineTest('bare import #1',
  `
import 'foo';
require('foo');
`,
  `
import "foo";
`,
)

inlineTest('bare import #2',
`
require('foo');
require('foo');
`,
  `
import "foo";
`,
)

inlineTest('bare import #3',
  `
require('foo');
var foo = require('foo');
`,
  `
import foo from "foo";
`,
)

inlineTest('dynamic import #1',
  `
var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");
Promise.resolve().then(() => require('foo'));
Promise.resolve().then(() => _interopRequireWildcard(require('bar')));
`,
  `
import("foo");
import("bar");
`,
)

inlineTest('require with destructuring and property access',
  `
var { bar } = require('foo').baz2;
var baz1 = require('foo').baz3;
`,
  `
import { baz2, baz3 as baz1 } from "foo";
var { bar } = baz2;
`,
)

inlineTest('require with property access and naming conflict',
  `
var { baz } = require('foo').bar;
var bar = 1;
console.log(bar);
`,
  `
import { bar as bar$0 } from "foo";
var { baz } = bar$0;
var bar = 1;
console.log(bar);
`,
)

inlineTest('multiple default import with same source',
  `
var foo = require('foo');
var bar = require('foo');
`,
  `
import foo from "foo";
import bar from "foo";
`,
)

inlineTest('multiple named import with same source',
  `
var { foo } = require('foo');
var { bar } = require('foo');
var baz = require('foo').baz;
`,
  `
import { foo, bar, baz } from "foo";
`,
)

inlineTest('import mixed with requires',
  `
import bar from 'bar';

var foo = require('bar');
var bro = require('bar').baz;
`,
  `
import bar, { baz as bro } from "bar";
import foo from "bar";
`,
)

inlineTest('requires that are not on top level should not be transformed',
  `
function fn() {
  require('foo');
  var bar = require('bar');
  var baz = require('baz').baz;
  return bar + baz;
}
`,
  `
function fn() {
  require('foo');
  var bar = require('bar');
  var baz = require('baz').baz;
  return bar + baz;
}
`,
)

inlineTestOpt('require should be hoisted #1', { hoist: true },
  `
function fn() {
  require('foo');
  var bar = require('bar');
  var baz = require('baz').baz;
  return bar + baz;
}
`,
  `
import "foo";
import bar from "bar";
import { baz } from "baz";
function fn() {
  return bar + baz;
}
`,
)

inlineTestOpt('require should be hoisted #2', { hoist: true },
  `
function fn() {
  var bar = 1;
  var { baz } = require('foo').bar;
  return baz;
}
`,
  `
import { bar as bar$0 } from "foo";
function fn() {
  var bar = 1;
  var { baz } = bar$0;
  return baz;
}
`,
)

inlineTestOpt('require should be hoisted #3', { hoist: true },
  `
var bar = 1;
function fn() {
  var { baz } = require('foo').bar;
  return baz;
}
`,
  `
import { bar as bar$0 } from "foo";
var bar = 1;
function fn() {
  var { baz } = bar$0;
  return baz;
}
`,
)

inlineTestOpt('nameless require #1', { hoist: true },
  `
var foo = require("bar")("baz");
var buz = require("bar").bar("baz");
`,
  `
import bar from "bar";
var foo = bar("baz");
var buz = bar.bar("baz");
`,
)

inlineTestOpt('nameless require #2', { hoist: true },
  `
var foo = require("foo")("baz");
var buz = require("foo").bar("baz");
`,
  `
import foo$0 from "foo";
var foo = foo$0("baz");
var buz = foo$0.bar("baz");
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

inlineTest('named exports strategy #1',
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

inlineTest('named exports strategy #2',
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

inlineTest('duplicate default exports #1',
  `
module.exports = 1;
module.exports = 2;
`,
  `
export default 2;
`,
)

inlineTest('duplicate default exports #2',
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

inlineTest('variable declaration with default exports #1',
  `
var foo = exports.default = 1;
`,
  `
var foo = 1;
export default foo;
`,
)

inlineTest('variable declaration with default exports #2',
  `
var foo = module.exports.default = 1;
`,
  `
var foo = 1;
export default foo;
`,
)

//  TODO:
// inlineTest('Object.defineProperty with exports',
//   `
// Object.defineProperty(exports, "foo", { value: 1 });
// Object.defineProperty(exports, "named", {
//   enumerable: true,
//   get: function () {
//     return obj.named;
//   }
// });
// `,
//   `
// export const foo = 1;
// export const named = obj.named;
// `,
// )

inlineTest('export with naming conflict #1',
  `
var foo = 1;
console.log(foo);
exports.foo = 2;

const bar = 2;
const bar$0 = 3;
console.log(bar, bar$0);
module.exports.bar = 4;
`,
  `
var foo = 1;
console.log(foo);
const foo$0 = 2;
export { foo$0 as foo };

const bar = 2;
const bar$0 = 3;
console.log(bar, bar$0);
const bar$1 = 4;
export { bar$1 as bar };
`,
)

inlineTest('export with naming conflict #2',
  `
var foo = 1;
var bar = 2;
console.log('foo', foo);
console.log('bar', bar);
exports.foo = bar;

const baz = 3;
const qux = 4;
console.log('baz', baz);
console.log('qux', qux);
module.exports.baz = qux;
`,
  `
var foo = 1;
var bar = 2;
console.log('foo', foo);
console.log('bar', bar);
export { bar as foo };

const baz = 3;
const qux = 4;
console.log('baz', baz);
console.log('qux', qux);
export { qux as baz };
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

/**
 * TODO: We might need a final pass to merge import and export
 *
 * The best result should be
 * ```js
 * export { default as foo } from "bar";
 */
inlineTestOpt('export with require #1', { hoist: true },
  `
module.exports.foo = require('bar');
`,
  `
import bar from "bar";
export const foo = bar;
`,
)

inlineTestOpt('export with require #2', { hoist: true },
  `
module.exports = require('bar');
`,
  `
import bar from "bar";
export default bar;
`,
)

inlineTestOpt('export with require #3', { hoist: true },
  `
var bar = 1;
module.exports = require('bar');
`,
  `
import bar$0 from "bar";
var bar = 1;
export default bar$0;
`,
)

/**
 * TODO: Not sure where to merge the short-hand property
 * Should be a new rule to handle this
 */
inlineTestOpt('export with require #4', { hoist: true },
  `
module.exports = {
  encode: require('encode'),
  decode: require('decode')
};
`,
  `
import encode from "encode";
import decode from "decode";

export default {
  encode: encode,
  decode: decode
};
`,
)
