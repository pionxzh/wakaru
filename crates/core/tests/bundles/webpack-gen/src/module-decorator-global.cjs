// Reads both `global` and `module` as values, so webpack 4 injects both its
// `buildin/global.js` value and its `buildin/module.js` decorator.
var freeGlobal = typeof global === "object" && global && global.Object === Object && global;
var freeModule = typeof module === "object" && module && !module.nodeType && module;
var root = freeGlobal || Function("return this")();

function second() {
  return root === freeGlobal;
}

if (freeModule) {
  freeModule.exports = second;
} else {
  root.second = second;
}
