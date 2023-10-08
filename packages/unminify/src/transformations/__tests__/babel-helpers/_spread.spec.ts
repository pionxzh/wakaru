import { defineInlineTest } from '@wakaru/test-utils'

// let's take objectSpread as an example to test, but the underlying implementation is the same for all spread-like helpers
import transform from '../../runtime-helpers/babel/objectSpread'

const inlineTest = defineInlineTest(transform)

inlineTest('objectSpread',
  `
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");

a = _objectSpread2({}, y);
b = _objectSpread2.default({}, y);
c = (0, _objectSpread2)({}, y);
d = (0, _objectSpread2.default)({}, y);
`,
  `
a = {
  ...y
};
b = {
  ...y
};
c = {
  ...y
};
d = {
  ...y
};
`,
)

inlineTest('objectSpread - esm',
  `
import _objectSpread2 from "@babel/runtime/helpers/esm/objectSpread2";

a = _objectSpread2({}, y);
b = _objectSpread2.default({}, y);
c = (0, _objectSpread2)({}, y);
d = (0, _objectSpread2.default)({}, y);
`,
  `
a = {
  ...y
};
b = {
  ...y
};
c = {
  ...y
};
d = {
  ...y
};
`,
)

inlineTest('objectSpread - cases',
  `
import _objectSpread2 from "@babel/runtime/helpers/esm/objectSpread";

a = _objectSpread2({}, y);
b = _objectSpread2({ x }, y);
c = _objectSpread2({ x: x }, y);
d = _objectSpread2({ x: z }, { y: 'bar'});
e = _objectSpread2({}, { get y() {} });
f = _objectSpread2({ x }, { y: _objectSpread2({}, z) });
g = _objectSpread2(
  _objectSpread2(
    _objectSpread2(
      { a },
      b
    ),
    {},
    { c },
    d
  ),
  {},
  { e }
);
`,
  `
a = {
  ...y
};
b = {
  x,
  ...y
};
c = {
  x: x,
  ...y
};
d = {
  x: z,
  y: 'bar'
};
e = {
  get y() {}
};
f = {
  x,

  y: {
    ...z
  }
};
g = {
  a,
  ...b,
  c,
  ...d,
  e
};
`,
)

inlineTest('objectSpread - with objectDestructuringEmpty',
  `
var _objectSpread2 = require("@babel/runtime/helpers/objectSpread2");
var _objectDestructuringEmpty = require("@babel/runtime/helpers/objectDestructuringEmpty");

a = _objectSpread2({}, (_objectDestructuringEmpty(y), y));
`,
  `
a = {
  ...y
};
`,
)
