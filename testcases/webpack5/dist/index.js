/******/ (() => { // webpackBootstrap
/******/ 	"use strict";
/******/ 	var __webpack_modules__ = ({

/***/ "./src/1.js":
/*!******************!*\
  !*** ./src/1.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   "default": () => (/* binding */ m1)
/* harmony export */ });
function m1() {
    console.log('m1')
}


/***/ }),

/***/ "./src/a.js":
/*!******************!*\
  !*** ./src/a.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   A: () => (/* binding */ A),
/* harmony export */   A_A: () => (/* binding */ A_A)
/* harmony export */ });
class A {
    constructor() {
        this.label = 'a'
    }

    print() {
        console.log('a', this.version)
    }
}

class A_A {
    constructor() {
        this.label = 'a_a'
    }
}


/***/ }),

/***/ "./src/b.js":
/*!******************!*\
  !*** ./src/b.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   "default": () => (/* export default binding */ __WEBPACK_DEFAULT_EXPORT__),
/* harmony export */   version: () => (/* binding */ version)
/* harmony export */ });
const version = '1.0.0'

/* harmony default export */ function __WEBPACK_DEFAULT_EXPORT__() {
    console.log('b', version)
}


/***/ }),

/***/ "./src/c.js":
/*!******************!*\
  !*** ./src/c.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   getC: () => (/* binding */ getC)
/* harmony export */ });
/* harmony import */ var _b_js__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(/*! ./b.js */ "./src/b.js");


const getC = async () => {
    console.log('c.a', _b_js__WEBPACK_IMPORTED_MODULE_0__.version)
    const result = await fetch('https://jsonplaceholder.typicode.com/todos/1')
    const json = await result.json()
    return json
}


/***/ }),

/***/ "./src/d.js":
/*!******************!*\
  !*** ./src/d.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   A: () => (/* reexport safe */ _a_js__WEBPACK_IMPORTED_MODULE_0__.A),
/* harmony export */   A_A: () => (/* reexport safe */ _a_js__WEBPACK_IMPORTED_MODULE_0__.A_A)
/* harmony export */ });
/* harmony import */ var _a_js__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(/*! ./a.js */ "./src/a.js");
// re-export



/***/ }),

/***/ "./src/e.js":
/*!******************!*\
  !*** ./src/e.js ***!
  \******************/
/***/ ((__unused_webpack___webpack_module__, __webpack_exports__, __webpack_require__) => {

__webpack_require__.r(__webpack_exports__);
/* harmony export */ __webpack_require__.d(__webpack_exports__, {
/* harmony export */   A: () => (/* reexport safe */ _a_js__WEBPACK_IMPORTED_MODULE_0__.A)
/* harmony export */ });
/* harmony import */ var _a_js__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(/*! ./a.js */ "./src/a.js");


// partial re-export



/***/ })

/******/ 	});
/************************************************************************/
/******/ 	// The module cache
/******/ 	var __webpack_module_cache__ = {};
/******/ 	
/******/ 	// The require function
/******/ 	function __webpack_require__(moduleId) {
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
/******/ 		__webpack_modules__[moduleId](module, module.exports, __webpack_require__);
/******/ 	
/******/ 		// Return the exports of the module
/******/ 		return module.exports;
/******/ 	}
/******/ 	
/************************************************************************/
/******/ 	/* webpack/runtime/define property getters */
/******/ 	(() => {
/******/ 		// define getter functions for harmony exports
/******/ 		__webpack_require__.d = (exports, definition) => {
/******/ 			for(var key in definition) {
/******/ 				if(__webpack_require__.o(definition, key) && !__webpack_require__.o(exports, key)) {
/******/ 					Object.defineProperty(exports, key, { enumerable: true, get: definition[key] });
/******/ 				}
/******/ 			}
/******/ 		};
/******/ 	})();
/******/ 	
/******/ 	/* webpack/runtime/hasOwnProperty shorthand */
/******/ 	(() => {
/******/ 		__webpack_require__.o = (obj, prop) => (Object.prototype.hasOwnProperty.call(obj, prop))
/******/ 	})();
/******/ 	
/******/ 	/* webpack/runtime/make namespace object */
/******/ 	(() => {
/******/ 		// define __esModule on exports
/******/ 		__webpack_require__.r = (exports) => {
/******/ 			if(typeof Symbol !== 'undefined' && Symbol.toStringTag) {
/******/ 				Object.defineProperty(exports, Symbol.toStringTag, { value: 'Module' });
/******/ 			}
/******/ 			Object.defineProperty(exports, '__esModule', { value: true });
/******/ 		};
/******/ 	})();
/******/ 	
/************************************************************************/
var __webpack_exports__ = {};
// This entry need to be wrapped in an IIFE because it need to be isolated against other modules in the chunk.
(() => {
/*!**********************!*\
  !*** ./src/index.js ***!
  \**********************/
__webpack_require__.r(__webpack_exports__);
/* harmony import */ var _1_js__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__(/*! ./1.js */ "./src/1.js");
/* harmony import */ var _a_js__WEBPACK_IMPORTED_MODULE_1__ = __webpack_require__(/*! ./a.js */ "./src/a.js");
/* harmony import */ var _b_js__WEBPACK_IMPORTED_MODULE_2__ = __webpack_require__(/*! ./b.js */ "./src/b.js");
/* harmony import */ var _c_js__WEBPACK_IMPORTED_MODULE_3__ = __webpack_require__(/*! ./c.js */ "./src/c.js");
/* harmony import */ var _d_js__WEBPACK_IMPORTED_MODULE_4__ = __webpack_require__(/*! ./d.js */ "./src/d.js");
/* harmony import */ var _e_js__WEBPACK_IMPORTED_MODULE_5__ = __webpack_require__(/*! ./e.js */ "./src/e.js");







const d = new _d_js__WEBPACK_IMPORTED_MODULE_4__.A()
const e = new _e_js__WEBPACK_IMPORTED_MODULE_5__.A()

console.log(_b_js__WEBPACK_IMPORTED_MODULE_2__.version, _a_js__WEBPACK_IMPORTED_MODULE_1__.A, d, e)

;(0,_b_js__WEBPACK_IMPORTED_MODULE_2__["default"])()

;(0,_c_js__WEBPACK_IMPORTED_MODULE_3__.getC)().then(console.log)

// const M1 = await import('./1.js')

;(0,_1_js__WEBPACK_IMPORTED_MODULE_0__["default"])()

})();

/******/ })()
;
//# sourceMappingURL=index.js.map