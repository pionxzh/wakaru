import { defineInlineTest } from '@wakaru/test-utils'
import unFlipComparisons from '../un-flip-comparisons'
import transform from '../un-parameters'

const inlineTest = defineInlineTest([unFlipComparisons, transform])

inlineTest('default parameters - Basic',
  `
function test() {
  if (x === void 0) x = 1;
  if (void 0 === y) { y = 2; }
  var b = arguments.length > 2 ? arguments[2] : undefined;
  var z = arguments.length > 3 && arguments[3] !== undefined ? arguments[3] : "hello";
  var e = arguments.length > 4 && undefined !== arguments[4]
    ? arguments[4]
    : world();
  var f = !(arguments.length > 5) || arguments[5] === undefined || arguments[5];
  console.log(x, y, b, z, e, f);
}
`,
  `
function test(x = 1, y = 2, b, z = "hello", e = world(), f = true) {
  console.log(x, y, b, z, e, f);
}
`,
)

inlineTest('default parameters - With gap',
  `
function test(a) {
  var b = arguments.length > 1 ? arguments[1] : undefined;
  var e = arguments.length > 4 && undefined !== arguments[4]
    ? arguments[4]
    : world();
  var z = arguments.length > 6 && arguments[6] !== undefined ? arguments[6] : "hello";
  var _param_3 = 1;
}
`,
  `
function test(a, b, _param_2, _param_3_1, e = world(), _param_5, z = "hello") {
  var _param_3 = 1;
}
`,
)

inlineTest('default parameters - ArrowFunctionExpression',
  `
const test = (a, b) => {
  if (a === void 0) a = 1;
  if (void 0 === b) b = 2;
}
`,
  `
const test = (a = 1, b = 2) => {}
`,
)

inlineTest('default parameters - ObjectMethod',
  `
const test = {
  foo(a, b) {
    if (a === void 0) a = 1;
    if (void 0 === b) b = 2;
  }
}
`,
  `
const test = {
  foo(a = 1, b = 2) {}
}
`,
)

inlineTest('default parameters - ClassMethod',
  `
class Test {
  foo(a, b) {
    if (a === void 0) a = 1;
    if (void 0 === b) b = 2;
  }
}
`,
  `
class Test {
  foo(a = 1, b = 2) {}
}
`,
)

inlineTest('Babel: default before last',
  `
function foo() {
  var a = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "foo";
  var b = arguments.length > 1 ? arguments[1] : undefined;
}
`,
  `
function foo(a = "foo", b) {}
`,
)

inlineTest('Babel: default iife self',
  `
var Ref = /*#__PURE__*/function () {
  "use strict";

  function Ref() {
    var ref = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : Ref;
    babelHelpers.classCallCheck(this, Ref);
    this.ref = ref;
  }
  return babelHelpers.createClass(Ref);
}();
var X = /*#__PURE__*/function () {
  "use strict";

  function X() {
    var x = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : foo;
    babelHelpers.classCallCheck(this, X);
    this.x = x;
  }
  return babelHelpers.createClass(X);
}();
`,
  `
var Ref = /*#__PURE__*/function () {
  "use strict";

  function Ref(ref = Ref) {
    babelHelpers.classCallCheck(this, Ref);
    this.ref = ref;
  }
  return babelHelpers.createClass(Ref);
}();
var X = /*#__PURE__*/function () {
  "use strict";

  function X(x = foo) {
    babelHelpers.classCallCheck(this, X);
    this.x = x;
  }
  return babelHelpers.createClass(X);
}();
`,
)

inlineTest('Babel: default multiple',
  `
var t = function () {
  var e = arguments.length > 0 && arguments[0] !== undefined ? arguments[0] : "foo";
  var f = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 5;
  return e + " bar " + f;
};

var a = function (e) {
  var f = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 5;
  return e + " bar " + f;
};
`,
  `
var t = function(e = "foo", f = 5) {
  return e + " bar " + f;
};

var a = function(e, f = 5) {
  return e + " bar " + f;
};
`,
)

inlineTest('Babel: default rest mix',
  `
function fn(a1) {
  var a2 = arguments.length > 1 && arguments[1] !== undefined ? arguments[1] : 4;
  var _ref = arguments.length > 2 ? arguments[2] : undefined;
  a3 = _ref.a3;
  a4 = _ref.a4;
  var a5 = arguments.length > 3 ? arguments[3] : undefined;
  var _ref2 = arguments.length > 4 && arguments[4] !== undefined ? arguments[4] : {};
  a6 = _ref2.a6;
  a7 = _ref2.a7;
}
`,
  `
function fn(a1, a2 = 4, _ref, a5, _ref2 = {}) {
  a3 = _ref.a3;
  a4 = _ref.a4;
  a6 = _ref2.a6;
  a7 = _ref2.a7;
}
`,
)

inlineTest('Babel: default setter',
  `
var obj = {
  set field(num) {
    if (num === void 0) {
      num = 1;
    }
    this.num = num;
  }
};
`,
  `
var obj = {
  set field(num = 1) {
    this.num = num;
  }
};
`,
)

inlineTest('Should not transform if parameter is used before default value',
  `
function test(a, b) {
  if (a === void 0) a = 1;
  console.log(b);
  if (b === void 0) b = 2;
}
`,
  `
function test(a = 1, b) {
  console.log(b);
  if (b === void 0) b = 2;
}
`,
)
