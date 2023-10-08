import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../../runtime-helpers/babel/interopRequireDefault'

const inlineTest = defineInlineTest(transform)

inlineTest('interopRequireDefault',
  `
import _source$es6Default from "source";

var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault");

_interopRequireDefault(_a);
_b = _interopRequireDefault(require("b"));
var _c = _interopRequireDefault(require("c"));
var _d = _interopRequireDefault(require("d")).default;

var _source = _interopRequireDefault(_source$es6Default).default;
_source;
var _source2 = _interopRequireDefault(_source$es6Default);
_source2.default;
_source2["default"];

(0, _b.default)();
(0, _c.default)();
`,
  `
import _source$es6Default from "source";

_a;
_b = require("b");
var _c = require("c");
var _d = require("d");

var _source = _source$es6Default;
_source;
var _source2 = _source$es6Default;
_source2;
_source2;

_b();
_c();
`,
)

inlineTest('interopRequireDefault with require.default',
  `
var _interopRequireDefault = require("@babel/runtime/helpers/interopRequireDefault").default;
var _interopRequireDefault2 = _interopRequireDefault(require("@babel/runtime/helpers/interopRequireDefault"));
console.log(_interopRequireDefault2.default);
`,
  `
var _interopRequireDefault2 = require("@babel/runtime/helpers/interopRequireDefault");
console.log(_interopRequireDefault2);
`,
)
