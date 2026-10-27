// The bridge followed by a test on the new value; the loop keeps the `if`
// a statement, so a minifier folds the bridge into the `if` test.
var content = require("./exports-bridge-content.cjs");

exports = module.exports = content.locals || {};
if (exports.alpha) {
  for (var key in content.locals) {
    exports[key + "Copy"] = content.locals[key];
  }
}
