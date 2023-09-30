import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../../babel-helpers/slicedToArray'
import lebab from '../../lebab'
import smartInline from '../../smart-inline'

const inlineTest = defineInlineTest([transform, lebab, smartInline])

inlineTest('slicedToArray',
  `
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");

var _ref = _slicedToArray(a, 2);
var name = _ref[0];
var setName = _ref[1];

var _ref2 = _slicedToArray.default(a, 2);
var name2 = _ref2[0];
var setName2 = _ref2[1];

var _ref3 = (0, _slicedToArray)(a, 2);
var name3 = _ref3[0];
var setName3 = _ref3[1];

var _ref4 = (0, _slicedToArray.default)(a, 2);
var name4 = _ref4[0];
var setName4 = _ref4[1];
`,
  `
const [name, setName] = a;
const [name2, setName2] = a;
const [name3, setName3] = a;
const [name4, setName4] = a;
`,
)

inlineTest('slicedToArray - esm',
  `
import _slicedToArray from "@babel/runtime/helpers/esm/slicedToArray";

var _ref = _slicedToArray(a, 2);
var name = _ref[0];
var setName = _ref[1];

var _ref2 = _slicedToArray.default(a, 2);
var name2 = _ref2[0];
var setName2 = _ref2[1];

var _ref3 = (0, _slicedToArray)(a, 2);
var name3 = _ref3[0];
var setName3 = _ref3[1];

var _ref4 = (0, _slicedToArray.default)(a, 2);
var name4 = _ref4[0];
var setName4 = _ref4[1];
`,
  `
const [name, setName] = a;
const [name2, setName2] = a;
const [name3, setName3] = a;
const [name4, setName4] = a;
`,
)

inlineTest('slicedToArray - advanced',
  `
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");

var _ref = _slicedToArray(a, 0);

var _ref2 = _slicedToArray(b, 1);
var name = _ref2[0];

var _ref3 = _slicedToArray(rect.meta, 2);
var mass = _ref3[1];
var weight = _ref3[2];
`,
  // FIXME: lebab didn't transform this var to const, not big deal imo
  `
var [] = a;
const [name] = b;
const [, mass, weight] = rect.meta;
`,
)

inlineTest('slicedToArray - for...in',
  `
import _slicedToArray from "@babel/runtime/helpers/esm/slicedToArray";

for (var _ref in obj) {
  var _ref2 = _slicedToArray(_ref, 2);
  var name = _ref2[0];
  var value = _ref2[1];
  print("Name: " + name + ", Value: " + value);
}

for (var __ref of test.expectation.registers) {
  var __ref2 = _slicedToArray(__ref, 3);
  var name = __ref2[0];
  var before = __ref2[1];
  var after = __ref2[2];
}
`,
  // FIXME: hmm got a redundant temp variable `__ref2` here
  `
for (const _ref in obj) {
  const [name, value] = _ref;
  print(\`Name: \${name}, Value: \${value}\`);
}

for (const __ref of test.expectation.registers) {
  const __ref2 = __ref;
  const [name, before, after] = __ref2;
}
`,
)

inlineTest('slicedToArray - invalid',
  `
import _slicedToArray from "@babel/runtime/helpers/esm/slicedToArray";

_slicedToArray();
_slicedToArray(a);
_slicedToArray(a, 2, 3);
`,
  `
import _slicedToArray from "@babel/runtime/helpers/esm/slicedToArray";

_slicedToArray();
_slicedToArray(a);
_slicedToArray(a, 2, 3);
`,
)
