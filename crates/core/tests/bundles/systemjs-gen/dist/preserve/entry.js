System.register(['./dep.js'], (function (exports, module) {
  'use strict';
  var named, greet;
  return {
    setters: [function (module) {
      named = module.named;
      greet = module.default;
    }],
    execute: (function () {

      exports("run", run);

      const value = exports("value", named + 1);

      async function run() {
        const mod = await module.import('./lazy.js');
        return greet(value + mod.extra + module.meta.url.length);
      }

    })
  };
}));
