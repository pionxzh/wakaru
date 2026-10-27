// The reversed bridge at top level, followed by a write through `exports`.
function createSecond() {
  return { first: Date.now() > 0 };
}

module.exports = exports = createSecond();
exports.second = "second";
