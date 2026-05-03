var __defProp = Object.defineProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};

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

// wakaru-rs/tests/fixtures/esbuild-gen/src/helper.js
var helper_exports = {};
__export(helper_exports, {
  average: () => average,
  total: () => total
});
function normalize(arr) {
  return arr.map((x) => x / Math.max(...arr));
}
function total(arr) {
  return normalize(arr).reduce((a, b) => a + b, 0);
}
function average(arr) {
  return total(arr) / arr.length;
}

// wakaru-rs/tests/fixtures/esbuild-gen/src/entry-private-helper.js
var main = function() {
  return "entry";
};
export {
  helper_exports as helper,
  main,
  math_exports as math
};
