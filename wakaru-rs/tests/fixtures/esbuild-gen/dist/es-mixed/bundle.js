var __create = Object.create;
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getProtoOf = Object.getPrototypeOf;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __commonJS = (cb, mod) => function __require() {
  return mod || (0, cb[__getOwnPropNames(cb)[0]])((mod = { exports: {} }).exports, mod), mod.exports;
};
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  // If the importer is in node compatibility mode or this is not an ESM
  // file that has been converted to a CommonJS file using a Babel-
  // compatible transform (i.e. "__esModule" has not been set), then set
  // "default" to the CommonJS "module.exports" for node compatibility.
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));

// wakaru-rs/tests/fixtures/esbuild-gen/src/utils-cjs.cjs
var require_utils_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/utils-cjs.cjs"(exports) {
    exports.clamp = function(val, min, max) {
      return Math.min(Math.max(val, min), max);
    };
    exports.double = function(x) {
      return x * 2;
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/format-cjs.cjs
var require_format_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/format-cjs.cjs"(exports) {
    exports.padLeft = function(str, len, ch) {
      return String(ch || " ").repeat(Math.max(0, len - str.length)) + str;
    };
    exports.padRight = function(str, len, ch) {
      return str + String(ch || " ").repeat(Math.max(0, len - str.length));
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/validate-cjs.cjs
var require_validate_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/validate-cjs.cjs"(exports) {
    exports.isEmail = function(s) {
      return /^[^@]+@[^@]+$/.test(s);
    };
    exports.isNumber = function(s) {
      return !isNaN(parseFloat(s));
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/convert-cjs.cjs
var require_convert_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/convert-cjs.cjs"(exports) {
    exports.toUpper = function(s) {
      return s.toUpperCase();
    };
    exports.toLower = function(s) {
      return s.toLowerCase();
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/array-cjs.cjs
var require_array_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/array-cjs.cjs"(exports) {
    exports.unique = function(arr) {
      return [...new Set(arr)];
    };
    exports.flatten = function(arr) {
      return arr.flat(Infinity);
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/object-cjs.cjs
var require_object_cjs = __commonJS({
  "wakaru-rs/tests/fixtures/esbuild-gen/src/object-cjs.cjs"(exports) {
    exports.keys = function(obj) {
      return Object.keys(obj);
    };
    exports.values = function(obj) {
      return Object.values(obj);
    };
  }
});

// wakaru-rs/tests/fixtures/esbuild-gen/src/entry-mixed.js
var import_utils_cjs = __toESM(require_utils_cjs());
var import_format_cjs = __toESM(require_format_cjs());
var import_validate_cjs = __toESM(require_validate_cjs());
var import_convert_cjs = __toESM(require_convert_cjs());
var import_array_cjs = __toESM(require_array_cjs());
var import_object_cjs = __toESM(require_object_cjs());

// wakaru-rs/tests/fixtures/esbuild-gen/src/math.js
var math_exports = {};
__export(math_exports, {
  PI: () => PI,
  add: () => add,
  multiply: () => multiply
});
var PI = 3.14159;
function add(a, b) {
  return a + b;
}
function multiply(a, b) {
  return a * b;
}

// wakaru-rs/tests/fixtures/esbuild-gen/src/greet.js
var greet_exports = {};
__export(greet_exports, {
  farewell: () => farewell,
  greet: () => greet
});
function greet(name) {
  return `Hello, ${name}!`;
}
function farewell(name) {
  return `Goodbye, ${name}!`;
}

// wakaru-rs/tests/fixtures/esbuild-gen/src/entry-mixed.js
function main() {
  return (0, import_format_cjs.padLeft)((0, import_convert_cjs.toUpper)((0, import_validate_cjs.isEmail)("a@b") ? "yes" : "no"), 10) + (0, import_utils_cjs.clamp)(42, 0, 100) + (0, import_array_cjs.unique)([1, 1, 2]).length + (0, import_object_cjs.keys)({ a: 1 }).length;
}
export {
  greet_exports as greet,
  main,
  math_exports as math
};
