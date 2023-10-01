import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../../runtime-helpers/babel/arrayLikeToArray'

const inlineTest = defineInlineTest(transform)

inlineTest('arrayLikeToArray',
  `
var _arrayLikeToArray = require("@babel/runtime/helpers/arrayLikeToArray");

_arrayLikeToArray([1,,3]);
_arrayLikeToArray.default([1,,3]);
(0, _arrayLikeToArray)([1,,3]);
(0, _arrayLikeToArray.default)([1,,3]);
`,
  `
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
`,
)

inlineTest('arrayLikeToArray - esm',
  `
import _arrayLikeToArray from "@babel/runtime/helpers/esm/arrayLikeToArray";

_arrayLikeToArray([1,,3]);
_arrayLikeToArray.default([1,,3]);
(0, _arrayLikeToArray)([1,,3]);
(0, _arrayLikeToArray.default)([1,,3]);
`,
  `
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
`,
)
