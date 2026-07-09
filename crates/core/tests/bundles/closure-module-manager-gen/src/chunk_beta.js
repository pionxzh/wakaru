globalThis["sampleClosureComponent"] = function (value) {
  return globalThis["sampleClosureFirst"](value) + 3;
};
