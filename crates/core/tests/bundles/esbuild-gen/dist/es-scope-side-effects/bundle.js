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

// src/registry.js
var registry_exports = {};
__export(registry_exports, {
  lookup: () => lookup,
  register: () => register
});
var modules = {};
function register(name, mod) {
  modules[name] = mod;
}
function lookup(name) {
  return modules[name];
}
register("self", { loaded: true });
export {
  math_exports as math,
  registry_exports as registry
};
