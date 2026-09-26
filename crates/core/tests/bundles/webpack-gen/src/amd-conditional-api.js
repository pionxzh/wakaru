(function (host) {
  if (!host) return;
  function api(value) { return value * 2; }
  host.api = api;
  if (typeof module !== "undefined" && module.exports) {
    module.exports = api;
  }
  if (typeof define === "function" && define.amd) {
    define(function () { return api; });
  }
})(typeof window !== "undefined" ? window : null);
