var CHANGE = 'change';
var RESET = 'reset';

function Store(initial) {
  this._data = Object.assign({}, initial);
  this._subs = [];
}

Store.prototype.get = function(key) {
  return this._data[key];
};

Store.prototype.set = function(key, value) {
  var old = this._data[key];
  this._data[key] = value;
  if (old !== value) {
    this._notify(CHANGE, { key: key, old: old, value: value });
  }
};

Store.prototype.subscribe = function(fn) {
  this._subs.push(fn);
};

Store.prototype._notify = function(type, payload) {
  for (var i = 0; i < this._subs.length; i++) {
    this._subs[i](type, payload);
  }
};

exports.Store = Store;
exports.CHANGE = CHANGE;
exports.RESET = RESET;
