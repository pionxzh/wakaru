var __create = Object.create;
var __getProtoOf = Object.getPrototypeOf;
var __defProp = Object.defineProperty;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
function __accessProp(key) {
  return this[key];
}
var __toESMCache_node;
var __toESMCache_esm;
var __toESM = (mod, isNodeMode, target) => {
  var canCache = mod != null && typeof mod === "object";
  if (canCache) {
    var cache = isNodeMode ? __toESMCache_node ??= new WeakMap : __toESMCache_esm ??= new WeakMap;
    var cached = cache.get(mod);
    if (cached)
      return cached;
  }
  target = mod != null ? __create(__getProtoOf(mod)) : {};
  const to = isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target;
  for (let key of __getOwnPropNames(mod))
    if (!__hasOwnProp.call(to, key))
      __defProp(to, key, {
        get: __accessProp.bind(mod, key),
        enumerable: true
      });
  if (canCache)
    cache.set(mod, to);
  return to;
};
var __commonJS = (cb, mod) => () => (mod || cb((mod = { exports: {} }).exports, mod), mod.exports);

// src-cjs/math.cjs
var require_math = __commonJS((exports, module) => {
  function add(a, b) {
    return a + b;
  }
  function multiply(a, b) {
    return a * b;
  }
  module.exports = { add, multiply };
});

// src-cjs/logger.cjs
var require_logger = __commonJS((exports, module) => {
  class Logger {
    constructor(prefix) {
      this._prefix = prefix || "";
    }
    info(msg) {
      console.log("[INFO] " + this._prefix + msg);
    }
    warn(msg) {
      console.warn("[WARN] " + this._prefix + msg);
    }
  }
  module.exports = Logger;
  module.exports.Logger = Logger;
});

// src-cjs/entry-cjs.js
var import_math = __toESM(require_math(), 1);

// src-cjs/format.cjs
function capitalize(str) {
  if (!str)
    return "";
  return str.charAt(0).toUpperCase() + str.slice(1);
}
function truncate(str, maxLen) {
  if (str.length <= maxLen)
    return str;
  return str.slice(0, maxLen - 3) + "...";
}
var $capitalize = capitalize;
var $truncate = truncate;

// src-cjs/entry-cjs.js
var import_logger = __toESM(require_logger(), 1);

// src-cjs/store.cjs
var CHANGE = "change";
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
    this._notify(CHANGE, { key, old, value });
  }
};
Store.prototype.subscribe = function(fn) {
  this._subs.push(fn);
};
Store.prototype._notify = function(type, payload) {
  for (var i = 0;i < this._subs.length; i++) {
    this._subs[i](type, payload);
  }
};
var $Store = Store;
var $CHANGE = CHANGE;

// src-cjs/entry-cjs.js
var log = new import_logger.default("app: ");
async function main() {
  log.info("Starting app");
  const items = [10, 20, 30, 40, 50];
  const total = items.reduce((sum, n) => import_math.add(sum, n), 0);
  const avg = import_math.multiply(total, 1 / items.length);
  const label = $truncate($capitalize("average value"), 10);
  log.info(label + ": " + avg);
  const store = new $Store({ count: 0 });
  store.subscribe(function(type, payload) {
    if (type === $CHANGE) {
      log.info(payload.key + " = " + payload.value);
    }
  });
  store.set("count", total);
  return { total, avg, label };
}
export {
  main
};
