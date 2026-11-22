"use strict";
if (!globalThis.concatCwPolyfill) {
  globalThis.concatCwPolyfill = function polyfill() {
    return "polyfill";
  };
}
