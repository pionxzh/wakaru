const choose = require("./inner-umd-truthy.cjs");
const createValue = require("./inner-umd-return.cjs");

module.exports = {
  chosen: choose("value"),
  created: createValue(2),
  createdKind: createValue.kind,
};
