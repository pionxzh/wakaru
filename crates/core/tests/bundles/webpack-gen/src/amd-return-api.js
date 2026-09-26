(function () {
  var api = function (value) { return value + 1; };
  api.default = api;
  if (typeof module !== "undefined" && module !== null && module.exports != null) {
    module.exports = api;
  }
  if (typeof define === "function" && define.amd) {
    define([], function () { return api; });
  } else {
    this.returnApi = api;
  }
  api.version = "1.0";
}).call(this);
