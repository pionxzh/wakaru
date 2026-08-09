let value;

(function () {
  function createValue(value) {
    return { value };
  }

  createValue.kind = "generated";
  (value = function () {
    return createValue;
  }.apply(exports, [])) === undefined || (module.exports = value);
})();
