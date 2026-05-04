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

// src/constants.js
var constants_exports = {};
__export(constants_exports, {
  LABEL: () => LABEL,
  VALUE: () => VALUE
});
var VALUE = 7;
var LABEL = "test";
console.log(LABEL, VALUE);
export {
  constants_exports as constants,
  math_exports as math
};
