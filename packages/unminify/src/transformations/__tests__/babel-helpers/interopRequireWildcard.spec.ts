import { defineInlineTest } from '@wakaru/test-utils'
import transform from '../../runtime-helpers/babel/interopRequireWildcard'

const inlineTest = defineInlineTest(transform)

inlineTest('interopRequireWildcard',
  `
import _source$es6Default from "source";

var _interopRequireWildcard = require("@babel/runtime/helpers/interopRequireWildcard");

_interopRequireWildcard(_a);
_b = _interopRequireWildcard(require("b"));
_c = _interopRequireWildcard(_c, true);
var _d = _interopRequireWildcard(require("d"));
var _source = _interopRequireWildcard(_source$es6Default);

Promise.resolve().then(() => _interopRequireWildcard(require("foo")));
`,
  `
import _source$es6Default from "source";

_a/** @hint namespace-import */;
_b = require("b")/** @hint namespace-import */;
_c = _c/** @hint namespace-import */;
var _d = require("d")/** @hint namespace-import */;
var _source = _source$es6Default/** @hint namespace-import */;

Promise.resolve().then(() => require("foo")/** @hint namespace-import */);
`,
)
