System.register([], (function (exports) {
  'use strict';
  return {
    execute: (function () {

      exports("default", greet);

      const named = exports("named", 41);

      function greet(name) {
        return `hi ${name}`;
      }

    })
  };
}));
