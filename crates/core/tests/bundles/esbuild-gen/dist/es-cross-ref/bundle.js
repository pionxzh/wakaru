var __defProp = Object.defineProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};

// src/cross-ref/math.js
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

// src/cross-ref/format.js
var format_exports = {};
__export(format_exports, {
  formatProduct: () => formatProduct,
  formatSum: () => formatSum
});
function formatSum(a, b) {
  return a + " + " + b + " = " + add(a, b);
}
function formatProduct(a, b) {
  return a + " * " + b + " = " + multiply(a, b) + " (PI=" + PI + ")";
}

// src/cross-ref/greet.js
var greet_exports = {};
__export(greet_exports, {
  greetWithMath: () => greetWithMath
});
function greetWithMath(name) {
  return "Hello " + name + "! " + formatSum(1, 2);
}
export {
  format_exports as format,
  greet_exports as greet,
  math_exports as math
};
