import transform from '../../babel-helpers/arrayWithoutHoles'
import { defineInlineTest } from '../test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('arrayWithoutHoles',
  `
var _arrayWithoutHoles = require("@babel/runtime/helpers/arrayWithoutHoles");

_arrayWithoutHoles([1,,3]);
_arrayWithoutHoles.default([1,,3]);
(0, _arrayWithoutHoles)([1,,3]);
(0, _arrayWithoutHoles.default)([1,,3]);
`,
  `
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
`,
)

inlineTest('arrayWithoutHoles - esm',
  `
import _arrayWithoutHoles from "@babel/runtime/helpers/esm/arrayWithoutHoles";

_arrayWithoutHoles([1,,3]);
_arrayWithoutHoles.default([1,,3]);
(0, _arrayWithoutHoles)([1,,3]);
(0, _arrayWithoutHoles.default)([1,,3]);
`,
  `
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
[1, undefined, 3];
`,
)
