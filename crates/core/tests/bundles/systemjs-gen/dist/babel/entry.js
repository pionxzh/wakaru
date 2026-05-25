System.register(["./dep.js"], function (_export, _context) {
  "use strict";

  var greet, named, value;
  async function run() {
    const mod = await _context.import("./lazy.js");
    return greet(value + mod.extra + _context.meta.url.length);
  }
  _export("run", run);
  return {
    setters: [function (_depJs) {
      greet = _depJs.default;
      named = _depJs.named;
    }],
    execute: function () {
      _export("value", value = named + 1);
    }
  };
});