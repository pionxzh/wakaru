const staticValue = require("./static");

function loadLazy() {
  return import("./lazy");
}

function prefetchLazy() {
  return __prefetchImport("./lazy");
}

function maybeSyncLazy() {
  return require.unstable_importMaybeSync("./lazy");
}

const weakLazyId = require.resolveWeak("./lazy");

module.exports = {
  loadLazy,
  maybeSyncLazy,
  prefetchLazy,
  staticValue,
  weakLazyId,
};
