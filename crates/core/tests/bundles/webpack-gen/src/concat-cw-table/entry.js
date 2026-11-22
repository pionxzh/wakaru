// An ES module entry that require()s an ES module. With module
// concatenation, webpack wraps that module, and every module it imports, in
// the `__webpack_require__.cw` accessor; `lazy.js` keeps `shared.cjs` and
// `marker.js` in the module table.
require("./polyfill.cjs");
import { tag } from "./shared.cjs";
import { mark } from "./marker.js";
import * as widgets from "./widget.js";
const provider = require("./provider.js");

function run() {
  const before = provider.count;
  provider.bump();
  return [
    mark(tag("entry")),
    provider.describe("x"),
    before,
    provider.count,
    typeof provider.loadLazy,
    Object.keys(widgets).join(","),
    provider.kinds().join(","),
  ];
}

globalThis.concatCwTableSink = { run };
