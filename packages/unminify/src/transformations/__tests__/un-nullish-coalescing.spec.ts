import transform from '../un-nullish-coalescing'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('Babel',
  `
foo !== null && foo !== void 0 ? foo : "bar";

var _ref;
(_ref = foo !== null && foo !== void 0 ? foo : bar) !== null && _ref !== void 0 ? _ref : "quz";

// transform-in-default-destructuring
var _foo$bar;
var { qux = (_foo$bar = foo.bar) !== null && _foo$bar !== void 0 ? _foo$bar : "qux" } = {};

// transform-in-default-param
function foo(foo, qux = (_foo$bar => (_foo$bar = foo.bar) !== null && _foo$bar !== void 0 ? _foo$bar : "qux")()) {}
function bar(bar, qux = bar !== null && bar !== void 0 ? bar : "qux") {}

// transform-in-function
function foo2(opts) {
  var _opts$foo;
  var foo = (_opts$foo = opts.foo) !== null && _opts$foo !== void 0 ? _opts$foo : "default";
}

// transform-static-refs-in-default
function foo3(foo, bar = foo !== null && foo !== void 0 ? foo : "bar") {}

// transform-static-refs-in-function
function foo4() {
  var foo = this !== null && this !== void 0 ? this : {};
}
`,
  `
foo ?? "bar";

var _ref;
foo ?? bar ?? "quz";

// transform-in-default-destructuring
var _foo$bar;
var { qux = foo.bar ?? "qux" } = {};

// transform-in-default-param
function foo(foo, qux = (_foo$bar => foo.bar ?? "qux")()) {}
function bar(bar, qux = bar ?? "qux") {}

// transform-in-function
function foo2(opts) {
  var _opts$foo;
  var foo = opts.foo ?? "default";
}

// transform-static-refs-in-default
function foo3(foo, bar = foo ?? "bar") {}

// transform-static-refs-in-function
function foo4() {
  var foo = this ?? {};
}
`,
)

inlineTest('SWC',
  `
foo !== null && foo !== void 0 ? foo : "bar";

var _ref;
(_ref = foo !== null && foo !== void 0 ? foo : bar) !== null && _ref !== void 0 ? _ref : "quz";

// transform-in-default-destructuring
var _foo_bar;
var _ref1 = {}, _ref_qux = _ref1.qux, qux = _ref_qux === void 0 ? (_foo_bar = foo.bar) !== null && _foo_bar !== void 0 ? _foo_bar : "qux" : _ref_qux;

// transform-in-default-param
var _foo_bar1;
function foo(foo) {
  var qux = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : (_foo_bar1 = foo.bar) !== null && _foo_bar1 !== void 0 ? _foo_bar1 : "qux";
}
function bar(bar) {
  var qux = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : bar !== null && bar !== void 0 ? bar : "qux";
}

// transform-in-function
function foo2(opts) {
  var _opts_foo;
  var _$foo = (_opts_foo = opts.foo) !== null && _opts_foo !== void 0 ? _opts_foo : "default";
}

// transform-static-refs-in-default
function foo3(foo) {
  var _$bar = arguments.length > 1 && arguments[1] !== void 0 ? arguments[1] : foo !== null && foo !== void 0 ? foo : "bar";
}

// transform-static-refs-in-function
function foo4() {
  var _this;
  var _$foo = (_this = this) !== null && _this !== void 0 ? _this : {};
}
`,
  `
foo ?? "bar";

var _ref;
foo ?? bar ?? "quz";

// transform-in-default-destructuring
var _foo_bar;
var { qux = foo.bar ?? "qux" } = {};

// transform-in-default-param
var _foo_bar1;
function foo(foo, qux = foo.bar ?? "qux") {}
function bar(bar, qux = bar ?? "qux") {}

// transform-in-function
function foo2(opts) {
  var _opts_foo;
  var _$foo = opts.foo ?? "default";
}

// transform-static-refs-in-default
function foo3(foo, bar = foo ?? "bar") {}

// transform-static-refs-in-function
function foo4() {
  var _this;
  var _$foo = this ?? {};
}
`,
)

inlineTest('TypeScript',
  `
var _a;
(_a = foo !== null && foo !== void 0 ? foo : bar) !== null && _a !== void 0 ? _a : 'quz';
`,
  `
var _a;
foo ?? bar ?? 'quz';
`,
)
