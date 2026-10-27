const lib = require("./module-decorator-cjs.cjs");
const second = require("./module-decorator-global.cjs");
const { alpha, hasModule } = require("./module-decorator-esm.js");

console.log(lib.first, second(), alpha, hasModule);
