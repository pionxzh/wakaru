(function (global, factory) {
  if (typeof define === "function" && define.amd) {
    define(factory);
  } else if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    global.counterApi = factory();
  }
})(this, function () {
  var calls = 0;
  var api = function (value) {
    calls += 1;
    return value + calls;
  };
  var formats = {};
  api.register = function (name, formatter) {
    if (formats[name]) throw new Error("Formatter already registered: " + name);
    formats[name] = formatter;
  };
  api.format = function (name, value) { return formats[name](value); };
  api.register("identity", function (value) { return String(value); });
  api.sum = function () {
    return Array.prototype.slice.call(arguments).reduce(function (sum, value) {
      return sum + value;
    }, 0);
  };
  api.version = "1.0.0";
  return api;
});
