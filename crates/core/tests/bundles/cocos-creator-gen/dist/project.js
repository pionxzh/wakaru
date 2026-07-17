window.__require=function e(modules, cache, entries) {
  function load(id, jumped) {
    if (!cache[id]) {
      if (!modules[id]) {
        var parts = id.split("/");
        var basename = parts[parts.length - 1];
        if (!modules[basename]) {
          var currentRequire = typeof __require == "function" && __require;
          if (!jumped && currentRequire) return currentRequire(basename, true);
          if (previousRequire) return previousRequire(basename, true);
          throw new Error("Cannot find module '" + id + "'");
        }
        id = basename;
      }
      var module = cache[id] = { exports: {} };
      modules[id][0].call(module.exports, function(request) {
        return load(modules[id][1][request] || request);
      }, module, module.exports, e, modules, cache, entries);
    }
    return cache[id].exports;
  }
  var previousRequire = typeof __require == "function" && __require;
  for (var i = 0; i < entries.length; i++) load(entries[i]);
  return load;
}({"UIBase":[function(require,module,exports){
cc._RF.push(module, "uiBaseFixtureUuid", "UIBase");

exports.UIBase = class UIBase {};

cc._RF.pop();

},{}],"SampleActivityBase":[function(require,module,exports){
cc._RF.push(module, "sampleActivityBaseFixtureUuid", "SampleActivityBase");

const UIBase = require("../UIBase").UIBase;

function shadowed(require) {
  return require("../UIBase");
}

exports.SampleActivityBase = class SampleActivityBase extends UIBase {};
exports.shadowed = shadowed;

cc._RF.pop();

},{"../UIBase":"UIBase"}],"SampleActivityBinder":[function(require,module,exports){
cc._RF.push(module, "sampleActivityBinderFixtureUuid", "SampleActivityBinder");

const engine = require("cc");
module.exports = require("./SampleActivityBase");
exports.engine = engine;

cc._RF.pop();

},{"./SampleActivityBase":"SampleActivityBase","cc":"cc"}]},{},["SampleActivityBinder"]);
