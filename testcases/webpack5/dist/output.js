(() => {
  "use strict";
  var __webpack_modules__ = {
    "./src/1.js": (
      __unused_webpack___webpack_module__,
      __webpack_exports__,
      __webpack_require__
    ) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, {
        default: () => m1,
      });
      function m1() {
        console.log("m1");
      }
    },

    "./src/a.js": (
      __unused_webpack___webpack_module__,
      __webpack_exports__,
      __webpack_require__
    ) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, {
        A: () => A,
      });
      class A {
        constructor() {
          this.label = "a";
        }

        print() {
          console.log("a", this.version);
        }
      }
    },

    "./src/b.js": (
      __unused_webpack___webpack_module__,
      __webpack_exports__,
      __webpack_require__
    ) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, {
        default: () => __WEBPACK_DEFAULT_EXPORT__,
        version: () => version,
      });
      const version = "1.0.0";

      function __WEBPACK_DEFAULT_EXPORT__() {
        console.log("b", version);
      }
    },

    "./src/c.js": (
      __unused_webpack___webpack_module__,
      __webpack_exports__,
      __webpack_require__
    ) => {
      __webpack_require__.r(__webpack_exports__);
      __webpack_require__.d(__webpack_exports__, {
        getC: () => getC,
      });
      var _b_js__WEBPACK_IMPORTED_MODULE_0__ =
        __webpack_require__("./src/b.js");

      const getC = async () => {
        console.log("c.a", _b_js__WEBPACK_IMPORTED_MODULE_0__.version);
        const result = await fetch(
          "https://jsonplaceholder.typicode.com/todos/1"
        );
        const json = await result.json();
        return json;
      };
    },
  };
  var __webpack_module_cache__ = {};
  function __webpack_require__(moduleId) {
    var cachedModule = __webpack_module_cache__[moduleId];
    if (cachedModule !== undefined) {
      return cachedModule.exports;
    }
    var module = (__webpack_module_cache__[moduleId] = {
      exports: {},
    });
    __webpack_modules__[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  (() => {
    __webpack_require__.d = (exports, definition) => {
      for (var key in definition) {
        if (
          __webpack_require__.o(definition, key) &&
          !__webpack_require__.o(exports, key)
        ) {
          Object.defineProperty(exports, key, {
            enumerable: true,
            get: definition[key],
          });
        }
      }
    };
  })();
  (() => {
    __webpack_require__.o = (obj, prop) =>
      Object.prototype.hasOwnProperty.call(obj, prop);
  })();
  (() => {
    __webpack_require__.r = (exports) => {
      if (typeof Symbol !== "undefined" && Symbol.toStringTag) {
        Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
      }
      Object.defineProperty(exports, "__esModule", { value: true });
    };
  })();
  var __webpack_exports__ = {};
  (() => {
    __webpack_require__.r(__webpack_exports__);
    var _a_js__WEBPACK_IMPORTED_MODULE_0__ = __webpack_require__("./src/a.js");
    var _b_js__WEBPACK_IMPORTED_MODULE_1__ = __webpack_require__("./src/b.js");
    var _c_js__WEBPACK_IMPORTED_MODULE_2__ = __webpack_require__("./src/c.js");
    var _1_js__WEBPACK_IMPORTED_MODULE_3__ = __webpack_require__("./src/1.js");

    console.log(
      _b_js__WEBPACK_IMPORTED_MODULE_1__.version,
      _a_js__WEBPACK_IMPORTED_MODULE_0__.A
    );
    (0, _b_js__WEBPACK_IMPORTED_MODULE_1__["default"])();
    (0, _c_js__WEBPACK_IMPORTED_MODULE_2__.getC)().then(console.log);
    (0, _1_js__WEBPACK_IMPORTED_MODULE_3__["default"])();
  })();
})();
