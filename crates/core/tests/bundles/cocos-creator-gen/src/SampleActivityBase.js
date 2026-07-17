cc._RF.push(module, "sampleActivityBaseFixtureUuid", "SampleActivityBase");

const UIBase = require("../UIBase").UIBase;

function shadowed(require) {
  return require("../UIBase");
}

exports.SampleActivityBase = class SampleActivityBase extends UIBase {};
exports.shadowed = shadowed;

cc._RF.pop();
