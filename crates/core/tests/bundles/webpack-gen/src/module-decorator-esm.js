// An ES module that reads `module` as a value. webpack 4 passes such a module
// its `buildin/harmony-module.js` decorator.
export const alpha = "alpha";
export const hasModule = typeof module === "object" && Boolean(module);
