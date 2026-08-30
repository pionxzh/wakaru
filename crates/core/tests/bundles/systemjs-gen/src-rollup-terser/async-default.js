const promise = (async function () {
  function DefaultValue() {}
  DefaultValue.self = DefaultValue;
  return DefaultValue;
})();

export default promise;
promise.marker = true;
