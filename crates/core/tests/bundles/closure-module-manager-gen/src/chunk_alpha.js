globalThis["sampleClosureFirst"] = function (value) {
  return globalThis["sampleClosureBase"](value) * 2;
};
