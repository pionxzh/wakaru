var direct = require("./amd-return-api");
var conditional = require("./amd-conditional-api");
module.exports = {
  value: direct(4),
  version: direct.version,
  self: direct.default === direct,
  conditional: typeof conditional === "function" ? conditional(4) : "absent"
};
