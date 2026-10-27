// A library IIFE that publishes itself through the Node idiom
// `exports = module.exports = Lib` and then decorates `exports`.
(function (root) {
  function Lib(name) {
    this.name = name;
  }

  Lib.prototype.first = function () {
    return this.name;
  };

  if (typeof exports !== "undefined") {
    if (typeof module !== "undefined" && module.exports) {
      exports = module.exports = Lib;
    }
    exports.Lib = Lib;
  } else {
    root.Lib = Lib;
  }
})(this);
