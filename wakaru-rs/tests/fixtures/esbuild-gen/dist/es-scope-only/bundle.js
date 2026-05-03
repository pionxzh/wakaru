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
export {
  greet_exports as greet,
  math_exports as math
};
