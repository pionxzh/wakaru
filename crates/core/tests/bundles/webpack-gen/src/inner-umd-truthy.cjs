(function (root) {
  function choose(value) {
    return value;
  }

  choose.enabled = true;
  module.exports ? (module.exports = choose) : (root.syntheticChoose = choose);
})(typeof window === "object" ? window : this);
