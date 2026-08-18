System.register([], function (_export, _context) {
  "use strict";

  function item() {}
  _export("item", item);
  return {
    setters: [],
    execute: function () {
      _export("item", item = makeOne()).a = 1;
      _export("item", item = makeTwo()).b = 2;
    }
  };
});