window.__require = function e(t, n, r) {
  function s(o, u) {
    if (!n[o]) {
      if (!t[o]) {
        var b = o.split("/");
        b = b[b.length - 1];
        if (!t[b]) {
          var a = "function" == typeof __require && __require;
          if (!u && a) return a(b, !0);
          if (i) return i(b, !0);
          throw new Error("Cannot find module '" + o + "'");
        }
        o = b;
      }
      var f = n[o] = {
        exports: {}
      };
      t[o][0].call(f.exports, function(e) {
        var n = t[o][1][e];
        return s(n || e);
      }, f, f.exports, e, t, n, r);
    }
    return n[o].exports;
  }
  var i = "function" == typeof __require && __require;
  for (var o = 0; o < r.length; o++) s(r[o]);
  return s;
}({
  Helloworld: [ function(require, module, exports) {
    "use strict";
    cc._RF.push(module, "e1b90/rohdEk4SdmmEZANaD", "Helloworld");
    Object.defineProperty(exports, "__esModule", {
      value: true
    });
    var _a = cc._decorator, ccclass = _a.ccclass, property = _a.property;
    var Helloworld = function(_super) {
      __extends(Helloworld, _super);
      function Helloworld() {
        var _this = null !== _super && _super.apply(this, arguments) || this;
        _this.label = null;
        _this.text = "hello";
        return _this;
      }
      Helloworld.prototype.start = function() {
        this.label.string = this.text;
      };
      __decorate([ property(cc.Label) ], Helloworld.prototype, "label", void 0);
      __decorate([ property ], Helloworld.prototype, "text", void 0);
      Helloworld = __decorate([ ccclass ], Helloworld);
      return Helloworld;
    }(cc.Component);
    exports.default = Helloworld;
    cc._RF.pop();
  }, {} ],
  a: [ function(require, module, exports) {
    "use strict";
    cc._RF.push(module, "ca4eeim0ItGuo9J7+j3HU71", "a");
    Object.defineProperty(exports, "__esModule", {
      value: true
    });
    exports.bump = exports.counter = exports.Mode = exports.Shop = exports.Box = void 0;
    exports.Box = {
      read: function() {
        return exports.Box.value;
      },
      value: 1
    };
    var Shop;
    (function(Shop) {
      function make() {
        return "made";
      }
      Shop.make = make;
    })(Shop = exports.Shop || (exports.Shop = {}));
    var Mode;
    (function(Mode) {
      Mode[Mode["A"] = 0] = "A";
      Mode[Mode["B"] = 1] = "B";
    })(Mode = exports.Mode || (exports.Mode = {}));
    exports.counter = 0;
    function bump() {
      exports.counter += 1;
      return exports.counter;
    }
    exports.bump = bump;
    cc._RF.pop();
  }, {} ],
  b: [ function(require, module, exports) {
    "use strict";
    cc._RF.push(module, "6f8f1rR/LxEf5fnEVahRWgW", "b");
    Object.defineProperty(exports, "__esModule", {
      value: true
    });
    var a_1 = require("./a");
    var _a = cc._decorator, ccclass = _a.ccclass, property = _a.property;
    var Main = function(_super) {
      __extends(Main, _super);
      function Main() {
        var _this = null !== _super && _super.apply(this, arguments) || this;
        _this.label = "main";
        return _this;
      }
      Main.prototype.start = function() {
        a_1.bump();
        console.log(a_1.Box.read(), a_1.Shop.make(), a_1.Mode.B, a_1.counter);
      };
      __decorate([ property ], Main.prototype, "label", void 0);
      Main = __decorate([ ccclass ], Main);
      return Main;
    }(cc.Component);
    exports.default = Main;
    cc._RF.pop();
  }, {
    "./a": "a"
  } ]
}, {}, [ "Helloworld", "a", "b" ]);