import { defineInlineTest } from '@unminify-kit/test-utils'
import transform from '../../runtime-helpers/babel/createForOfIteratorHelper'
import slicedToArray from '../../runtime-helpers/babel/slicedToArray'

const inlineTest = defineInlineTest(transform)

inlineTest('createForOfIteratorHelper',
  `
var _createForOfIteratorHelper = require("@babel/runtime/helpers/createForOfIteratorHelper");

var _iterator = _createForOfIteratorHelper(arr), _step;
try {
  for (_iterator.s(); !(_step = _iterator.n()).done;) {
    var _result = _step.value;
  }
} catch (err) {
  _iterator.e(err);
} finally {
  _iterator.f();
}
`,
  `
for (var _result of arr)
  {}
`,
)

defineInlineTest([slicedToArray, transform])('createForOfIteratorHelper - with loop fn',
  `
var _slicedToArray = require("@babel/runtime/helpers/slicedToArray");
var _createForOfIteratorHelper = require("@babel/runtime/helpers/createForOfIteratorHelper");

var _iterator = _createForOfIteratorHelper(arr), _step;
try {
  var _loop = function _loop() {
    var _result = _slicedToArray(_step.value, 1),
      a = _result[0];
    a = 1;
    (function () {
      return a;
    });
  };
  for (_iterator.s(); !(_step = _iterator.n()).done;) {
    _loop();
  }
} catch (err) {
  _iterator.e(err);
} finally {
  _iterator.f();
}
`,
  `
for (var _result of arr) {
  var a = _result[0];
  a = 1;
  (function () {
    return a;
  });
}
`,
)
