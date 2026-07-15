/******/ (() => { // webpackBootstrap
/******/ 	var __webpack_modules__ = ({

/***/ 881:
/***/ ((module) => {

function formatName(name) {
  return name.trim().toUpperCase();
}

module.exports = { formatName };


/***/ }),

/***/ 582:
/***/ ((module, __unused_webpack_exports, __nccwpck_require__) => {

const bar = __nccwpck_require__(881);

function greet(name) {
  const formattedName = bar.formatName(name);
  return `Hello ${formattedName}!`;
}

module.exports = { greet };


/***/ })

/******/ 	});
/************************************************************************/
/******/ 	// The module cache
/******/ 	var __webpack_module_cache__ = {};
/******/
/******/ 	// The require function
/******/ 	function __nccwpck_require__(moduleId) {
/******/ 		// Check if module is in cache
/******/ 		var cachedModule = __webpack_module_cache__[moduleId];
/******/ 		if (cachedModule !== undefined) {
/******/ 			return cachedModule.exports;
/******/ 		}
/******/ 		// Create a new module (and put it into the cache)
/******/ 		var module = __webpack_module_cache__[moduleId] = {
/******/ 			// no module.id needed
/******/ 			// no module.loaded needed
/******/ 			exports: {}
/******/ 		};
/******/
/******/ 		// Execute the module function
/******/ 		var threw = true;
/******/ 		try {
/******/ 			__webpack_modules__[moduleId](module, module.exports, __nccwpck_require__);
/******/ 			threw = false;
/******/ 		} finally {
/******/ 			if(threw) delete __webpack_module_cache__[moduleId];
/******/ 		}
/******/
/******/ 		// Return the exports of the module
/******/ 		return module.exports;
/******/ 	}
/******/
/************************************************************************/
/******/ 	/* webpack/runtime/compat */
/******/
/******/ 	if (typeof __nccwpck_require__ !== 'undefined') __nccwpck_require__.ab = __dirname + "/";
/******/
/************************************************************************/
var __webpack_exports__ = {};
const foo = __nccwpck_require__(582);

console.log(foo.greet("wakaru"));

module.exports = __webpack_exports__;
/******/ })()
;