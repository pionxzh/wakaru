// A UMD-style wrapper that reads `module` as a value. webpack 4 passes such a
// CommonJS module its `buildin/module.js` decorator.
(function (root, factory) {
  if (typeof module === "object" && module && module.exports) {
    module.exports = factory();
  } else {
    root.Lib = factory();
  }
})(this, function () {
  return { first: "first" };
});
