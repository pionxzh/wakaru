import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../../babel-helpers/toConsumableArray'

const inlineTest = defineInlineTest(transform)

inlineTest('toConsumableArray',
  `
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");

_toConsumableArray(a);
_toConsumableArray.default(a);
(0, _toConsumableArray)(a);
(0, _toConsumableArray.default)(a);
`,
  `
[...a];
[...a];
[...a];
[...a];
`,
)

inlineTest('toConsumableArray - esm',
  `
import _toConsumableArray from "@babel/runtime/helpers/esm/toConsumableArray";

_toConsumableArray(a);
_toConsumableArray.default(a);
(0, _toConsumableArray)(a);
(0, _toConsumableArray.default)(a);
`,
  `
[...a];
[...a];
[...a];
[...a];
`,
)

inlineTest('toConsumableArray - invalid',
  `
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");

_toConsumableArray(a, b);
_toConsumableArray.default(a, b);
(0, _toConsumableArray)(a, b);
(0, _toConsumableArray.default)(a, b);
`,
  `
var _toConsumableArray = require("@babel/runtime/helpers/toConsumableArray");

_toConsumableArray(a, b);
_toConsumableArray.default(a, b);
(0, _toConsumableArray)(a, b);
(0, _toConsumableArray.default)(a, b);
`,
)
