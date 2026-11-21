/******/ (() => { // webpackBootstrap
/******/ 	"use strict";
/******/ 	// The require scope
/******/ 	const __webpack_require__ = {};
/******/
/************************************************************************/
/******/ 	/* webpack/runtime/concatenation wrap */
/******/ 	// wrap a concatenated module body as a lazy, memoized accessor; mod is
/******/ 	// set before the body runs so re-entrant calls (require cycles) observe
/******/ 	// the partial exports like Node.js
/******/ 	__webpack_require__.cw = (body) => {
/******/ 		var mod;
/******/ 		return () => {
/******/ 			if (body) {
/******/ 				var fn = body;
/******/ 				body = 0;
/******/ 				mod = { exports: {} };
/******/ 				fn.call(mod.exports, mod, mod.exports);
/******/ 			}
/******/ 			return mod.exports;
/******/ 		};
/******/ 	};
/******/
/************************************************************************/

// MODULE: ./src/concat-cw/helper.js
var helper_namespaceFn = /*#__PURE__*/__webpack_require__.cw((__unused_webpack_module, exports) => {

exports.describe = function describe(value) {
  return "helper:" + value;
};

});

// MODULE: ./src/concat-cw/provider.js
var provider_namespaceFn = /*#__PURE__*/__webpack_require__.cw((__unused_webpack_module, exports) => {
var __webpack_unused_export__;

// A CommonJS provider that an ES module imports. Production builds with
// module concatenation wrap it in the `__webpack_require__.cw` accessor.
exports.U9 = 0;
exports.$p = bump;
__webpack_unused_export__ = makeLabel;
function bump() {
  exports.U9 += 1;
}
function makeLabel(value) {
  return "label:" + value;
}
class Widget {
  constructor(name) {
    this.name = name;
  }
  label() {
    return makeLabel(this.name);
  }
}
exports.x0 = Widget;

});

// MODULE: ./src/concat-cw/relay.js
var relay_namespaceFn = /*#__PURE__*/__webpack_require__.cw((module) => {

// A CommonJS module that replaces its exports with another one's.
module.exports = (helper_namespaceFn());

});

;// ./src/concat-cw/provider.js
provider_namespaceFn();

;// ./src/concat-cw/relay.js
relay_namespaceFn();

;// ./src/concat-cw/pair-entry.js



function run() {
  const before = (provider_namespaceFn().U9);
  (0,provider_namespaceFn().$p)();
  return [new (provider_namespaceFn().x0)("consumer").label(), (0,helper_namespaceFn().describe)("y"), before, (provider_namespaceFn().U9)];
}

globalThis.concatCwSink = { run };

/******/ })()
;