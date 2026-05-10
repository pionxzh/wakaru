(() => {
  var modules = {
    2: (module, exports, require) => {
      "use strict";
      const lib = require(3);
      console.log(lib);
    },
    3: module => {
      "use strict";
      module.exports = 'foo'
    }
  };
  var installedModules = {};
  function __webpack_require__(moduleId) {
    var cachedModule = installedModules[moduleId];
    if (cachedModule !== undefined) {
      return cachedModule.exports;
    }
    var module = installedModules[moduleId] = {
      exports: {}
    };
    modules[moduleId](module, module.exports, __webpack_require__);
    return module.exports;
  }
  __webpack_require__.o = (obj, prop) => Object.prototype.hasOwnProperty.call(obj, prop);
  __webpack_require__.d = (exports, definition) => {
    for (var key in definition) {
      if (__webpack_require__.o(definition, key) && !__webpack_require__.o(exports, key)) {
        Object.defineProperty(exports, key, { enumerable: true, get: definition[key] });
      }
    }
  };
  __webpack_require__.r = exports => {
    if (typeof Symbol != "undefined" && Symbol.toStringTag) {
      Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
    }
    Object.defineProperty(exports, "__esModule", { value: true });
  };
  var entryModule = __webpack_require__(__webpack_require__.s = 2);
  module.exports = entryModule;
})();
