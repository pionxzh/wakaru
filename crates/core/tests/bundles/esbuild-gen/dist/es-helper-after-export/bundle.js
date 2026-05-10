var __defProp = Object.defineProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};

// src/math.js
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

// src/utils-after.js
var utils_after_exports = {};
__export(utils_after_exports, {
  compute: () => compute
});
function compute(a, b) {
  return normalize(a) + normalize(b);
}
function normalize(x) {
  return x / Math.abs(x) || 0;
}

// src/entry-helper-after-export.js
var main = function() {
  return "entry";
};
export {
  main,
  math_exports as math,
  utils_after_exports as utils
};
