exports.getGlobal = function() {
  return global;
};

exports.GlobalBox = class GlobalBox {
  constructor(value) {
    this.value = value;
  }
};

exports.globalState = {
  get current() {
    return this._current;
  },
  set current(value) {
    this._current = value;
  },
};
