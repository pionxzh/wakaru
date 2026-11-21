"use strict";
// A CommonJS provider that an ES module imports. Production builds with
// module concatenation wrap it in the `__webpack_require__.cw` accessor.
exports.count = 0;
exports.bump = bump;
exports.makeLabel = makeLabel;
function bump() {
  exports.count += 1;
}
function makeLabel(value) {
  return "label:" + value;
}
class Widget {
  constructor(name) {
    this.name = name;
  }
  label() {
    return makeLabel(this.name);
  }
}
exports.Widget = Widget;
